//! lobby-3 (user: "if we buy certain weapons we should see them appear in game too"): the whole chain in the Rust
//! client against the Rust lobby and game server. The menu (scripted, headless, the ROM's layouts) logs in, opens the
//! shop, picks a GP primary weapon, presses Buy: the 2009 purchase window opens with the item's periods; OK sends
//! 0x328 and the lobby's 0x329 brings the item into the inventory (0x320 / 0x321 again) with the new GP. The purchase
//! box's OK asks "equip now or keep in storage"; Equip sends 0x324 and 0x323 shows it in slot 0; then the menu hosts a room, a dc-botclient walker joins
//! and the match on dc-dserver grants the bought weapon in slot 0. Every process is stopped by its own PID.
//!
//! ROM-gated (skipped without `CA_ROM`). `DC_MENU_SHOTS` names a folder for the PNGs of the shop with the purchase
//! window and of the storage page with the equipped weapon (ROM-derived: local files, never committed).
// SPEC: PROTOCOL_2009.md §4.2 (0x328 / 0x329, 0x324 / 0x325), §3.3 (0x320 .. 0x323); DSERVER_PIPE.md §3.1 (user
// line `loadout=`); GAME_MESSAGES_2009_S2C.md row 0xb2 (weapon grant)

mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use dc_botclient::motion::MotionKind;
use dc_botclient::record::Recorder;
use dc_botclient::run::{run_bot, BotConfig, RoomChoice};
use dc_client::menu::buybox;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::pilot::{GameBack, Pilot, PilotPlan, PilotRoom};
use dc_client::menu::shop::{
    grid_cells, CMD_BUY, LEFT_WINDOW, SHOP_GRID, SHOP_WINDOW, WEAPON_LIST,
};
use dc_client::menu::storage::{own_slot, Wares, TAB_WEAPONS};
use dc_client::menu::{pages, Menu, MenuInput, MenuOptions, MenuOutcome};
use dc_client::steps::{resolve, Resolved};
use dc_client::testkit::{synth_font, synth_source, SYNTH_DIMS};
use dc_client::{Connect, HeadlessOptions, NetOptions, WorldFile};
use dc_ui_data::WidgetKind;
use support::*;

const PASSWORD: &str = "pw-shop-buy";

fn tap(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Move(p));
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

fn mid(c: [i32; 4]) -> [f32; 2] {
    [(c[0] + c[2] / 2) as f32, (c[1] + c[3] / 2) as f32]
}

/// Runs the menu (no input) until `done` or `t` runs out.
fn pump(m: &mut Menu, t: Duration, mut done: impl FnMut(&Menu) -> bool) -> bool {
    let end = Instant::now() + t;
    while Instant::now() < end {
        m.update();
        if done(m) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn shot(m: &mut Menu, dir: &Path, name: &str) {
    let _ = std::fs::create_dir_all(dir);
    let img = m.image();
    let frame = dc_render::FrameImage {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    };
    let p = dir.join(name);
    match frame.save_png(&p) {
        Ok(()) => println!("shot: {}", p.display()),
        Err(e) => println!("shot {name}: {e}"),
    }
}

fn button_mid(m: &Menu, cmd: i32) -> [f32; 2] {
    let b = m
        .sheet()
        .button(cmd)
        .filter(|b| b.shown)
        .unwrap_or_else(|| panic!("button {cmd} not shown on {}", m.screen_name()));
    [
        (b.pos[0] + b.size[0] / 2) as f32,
        (b.pos[1] + b.size[1] / 2) as f32,
    ]
}

/// Lobby (+ dc-dserver for the made-up world) on loopback ports, GP for the purchases.
fn start_lobby(dir: &Path, watchdog: &Watchdog) -> (Proc, SocketAddr, PathBuf) {
    let world = synth_world_file(dir);
    let log = dir.join("lobby.log");
    let (port, lport, cport, web) = loop {
        let p = (
            free_tcp_port(),
            free_tcp_port(),
            free_tcp_port(),
            free_tcp_port(),
        );
        let mut v = vec![p.0, p.1, p.2, p.3, p.1.wrapping_add(1)];
        v.sort();
        v.dedup();
        if v.len() == 5 {
            break p;
        }
    };
    let dargs = format!(
        "-worldfile {} -announce Worlds\\Synth.dat -tick 20 -roundsecs 12 -holds 1500,800",
        world.display()
    );
    let mut cmd = Command::new(lobby_exe());
    cmd.args([
        "--port",
        &port.to_string(),
        "--lobbyport",
        &lport.to_string(),
    ])
    .args([
        "--clanport",
        &cport.to_string(),
        "--clanweb",
        &web.to_string(),
    ])
    .arg("--db")
    .arg(dir.join("lobby.db"))
    .args([
        "--countdown",
        "0.05",
        "--gameport",
        &free_udp_port().to_string(),
    ])
    .args(["--start-gp", "1000000"])
    .arg("--log")
    .arg(&log)
    .arg("--dserver")
    .arg("--dserver-exe")
    .arg(dserver_exe())
    .arg("--dserver-logdir")
    .arg(dir)
    .args(["--dserver-world", "Worlds\\Synth", "--dserver-args", &dargs])
    .current_dir(dir);
    let lob = Proc::spawn(cmd);
    watchdog.watch_pid(lob.pid());
    watchdog.watch_log(&log);
    let up = (0..600).any(|_| {
        std::thread::sleep(Duration::from_millis(50));
        std::net::TcpStream::connect(("127.0.0.1", lport)).is_ok()
            && std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
    });
    assert!(up, "lobby did not listen");
    (lob, format!("127.0.0.1:{port}").parse().unwrap(), log)
}

#[test]
fn rom_a_weapon_bought_in_the_purchase_window_and_equipped_is_carried_into_the_match() {
    let Some(rom) = dc_rom::resolve_rom().ok() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let watchdog = Watchdog::arm("menu-shop-buy", Duration::from_secs(300));
    let dir = scratch("menu-shop-buy");
    let shots = std::env::var_os("DC_MENU_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| dir.join("shots"));
    let (mut lob, auth, log) = start_lobby(&dir, &watchdog);
    let nick = "ShopBuyer";
    let mut link = LinkOptions::new(nick);
    link.log = Some(dir.join("menu_events.jsonl"));
    let mut m = Menu::new(MenuOptions {
        auth,
        user: nick.into(),
        link,
        rom: Some(rom.clone()),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.set_idle_rules(false);
    let mut pilot = Pilot::new(PilotPlan {
        password: PASSWORD.into(),
        room: PilotRoom::Create {
            title: "bought kit".into(),
            mode: None,
            map: None,
            start_after: 2,
        },
        matches: 1,
        shots: None,
        timeout: Duration::from_secs(120),
    });
    // the pilot logs in up to the lobby
    let end = Instant::now() + Duration::from_secs(60);
    while m.screen_name() != "lobby" {
        assert!(Instant::now() < end, "no lobby; at {}", m.screen_name());
        m.update();
        for i in pilot.next(&m) {
            m.input(i);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(pump(&mut m, Duration::from_secs(5), |m| !m
        .data
        .store
        .items
        .is_empty()));
    let gp0 = m.data.profile.gp;
    // Shop / Storage
    let Resolved::Part { at, .. } = resolve(&m, "lobby.shop") else {
        panic!("no shop button");
    };
    tap(&mut m, at);
    assert!(pump(&mut m, Duration::from_secs(10), |m| m.screen_name()
        == "shop"
        && m.data.store.view.is_some()));
    // a GP primary weapon on the first shop page that the character does not own yet
    let wares = Wares::cached(Some(&rom));
    let shelf = wares.shelf(&m.data.store, TAB_WEAPONS, 0);
    let owned: Vec<u32> = m.data.store.items.iter().map(|r| r.item_no).collect();
    let Some((cell, ware)) = shelf.iter().take(6).enumerate().find(|(_, w)| {
        w.sale_type == 0
            && own_slot(&wares, w.item_no as u32) == Some(0)
            && !owned.contains(&(w.item_no as u32))
            && i64::from(w.price) <= i64::from(gp0)
    }) else {
        panic!("no GP primary weapon on the first page: {shelf:?}");
    };
    let ware = ware.clone();
    println!(
        "buying {} (item {}, cell {cell}, {} GP)",
        ware.name, ware.item_no, ware.price
    );
    let cells = grid_cells(&m.cat, pages::SHOP, SHOP_WINDOW, SHOP_GRID);
    tap(&mut m, mid(cells[cell]));
    let b = button_mid(&m, CMD_BUY);
    tap(&mut m, b);
    // the purchase window: its period list holds the item's products; the first row is chosen
    let offers = buybox::offers(&wares, &m.data.store, ware.item_no, 0);
    assert!(!offers.is_empty(), "the window lists the item's products");
    assert_eq!(
        m.popup_count(),
        0,
        "Buy opened the purchase window, not a box"
    );
    pump(&mut m, Duration::from_millis(300), |_| false);
    shot(&mut m, &shots, "rust_shop_buy_window.png");
    // the period list opens under its field
    let arrow = [
        (buybox::WINDOW_AT[0] + buybox::ARROW_AT[0] + 7) as f32,
        (buybox::WINDOW_AT[1] + buybox::ARROW_AT[1] + 7) as f32,
    ];
    tap(&mut m, arrow);
    pump(&mut m, Duration::from_millis(200), |_| false);
    shot(&mut m, &shots, "rust_shop_buy_periods.png");
    tap(&mut m, arrow);
    let sent = m.link_sent();
    let ok = [
        (buybox::WINDOW_AT[0] + buybox::OK_AT[0] + buybox::BUTTON_SIZE[0] / 2) as f32,
        (buybox::WINDOW_AT[1] + buybox::OK_AT[1] + buybox::BUTTON_SIZE[1] / 2) as f32,
    ];
    tap(&mut m, ok);
    assert!(m.link_sent() > sent, "OK sent nothing");
    let req = m.link_latest_request();
    assert!(
        req.starts_with(&format!("BuyWith({}, 0,", offers[0].product)),
        "0x328 for the chosen product: {req}"
    );
    // 0x329 -> GP after, the inventory read again
    let item = ware.item_no as u32;
    assert!(
        pump(&mut m, Duration::from_secs(10), |m| m
            .data
            .store
            .items
            .iter()
            .any(|r| r.item_no == item)),
        "the bought item is in the inventory; lobby log:\n{}",
        std::fs::read_to_string(&log).unwrap_or_default()
    );
    assert_eq!(
        m.data.profile.gp,
        gp0 - offers[0].price,
        "GP after the purchase"
    );
    let bought = m.popup().map(|p| p.text.clone()).unwrap_or_default();
    assert_eq!(bought, m.cat.text(0x439), "the purchase box");
    assert!(
        m.popup().is_some_and(|p| p.ok_icon),
        "CLI-912: no warning icon on a success box"
    );
    shot(&mut m, &shots, "rust_shop_bought_box.png");
    // its OK asks to equip the new item now or keep it in storage (2009 boxes 0x439 -> 0x36b); Equip sends 0x324
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    assert!(pump(&mut m, Duration::from_secs(2), |m| m
        .popup()
        .is_some_and(|p| p.text == m.cat.text(0x36b))));
    shot(&mut m, &shots, "rust_shop_equip_ask.png");
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Enter));
    // storage-functions (CLI-772): slot 0 is filled and the default backpack has a weapon slot, so 2009 asks for the
    // slot in window 7 first (cshell+0x148020 -> cshell+0x14f180); its OK on the checked Weapon Slot equips into slot 0
    if m.sheet()
        .button(dc_client::menu::slotpick::CMD_OK)
        .is_some()
    {
        let ok = button_mid(&m, dc_client::menu::slotpick::CMD_OK);
        tap(&mut m, ok);
    }
    assert!(
        pump(&mut m, Duration::from_secs(10), |m| m.data.store.equipped
            [0]
            == item as i32),
        "0x323 shows the bought weapon in slot 0: {:?}",
        m.data.store.equipped
    );
    assert!(
        m.popup().is_none(),
        "no refusal: {:?}",
        m.popup().map(|p| &p.text)
    );
    // the storage page of slot 0 lists it as equipped
    let l = m
        .sheet()
        .part(LEFT_WINDOW, WEAPON_LIST, WidgetKind::List)
        .expect("weapon list")
        .pos;
    tap(&mut m, [l[0] as f32 + 10.0, l[1] as f32 + 10.0]);
    let stored = dc_client::menu::storage::stored(&m.data.store, &wares, TAB_WEAPONS, 0);
    assert!(
        stored.iter().any(|r| r.item_no == item),
        "the bought item on the storage page"
    );
    pump(&mut m, Duration::from_millis(300), |_| false);
    shot(&mut m, &shots, "rust_storage_equipped.png");
    m.input(MenuInput::Key(dc_client::menu::MenuKey::Escape));
    assert!(pump(&mut m, Duration::from_secs(5), |m| m.screen_name()
        == "lobby"));
    pump(&mut m, Duration::from_millis(500), |_| false);
    shot(&mut m, &shots, "rust_lobby_after_buy.png");

    // the next match: host a room, a walker joins, the game server grants the weapons
    let bot = std::thread::spawn(move || {
        let mut cfg = BotConfig::new(auth, "ShopWalker");
        cfg.room = RoomChoice::Any;
        cfg.motion = MotionKind::Circle;
        cfg.duration = Duration::from_secs(20);
        cfg.use_rom = false;
        cfg.load_ms = 300;
        cfg.match_wait = Duration::from_secs(40);
        run_bot(&cfg, &Recorder::null("ShopWalker"))
    });
    let mut base = NetOptions::new(
        Connect::Lobby {
            auth,
            room: RoomChoice::Any,
            password: String::new(),
            channel: None,
        },
        nick,
    );
    base.world_file = Some({
        let s = synth_source();
        WorldFile {
            bytes: s.bytes,
            stand_dims: SYNTH_DIMS,
            textures: s.textures,
        }
    });
    base.match_wait = Duration::from_secs(30);
    base.result_hold = Duration::from_millis(1000);
    let hopts = HeadlessOptions {
        frames: 60 * 20,
        frame_us: 16_667,
        width: 320,
        height: 240,
        out: None,
        save_every: 12,
        font_bytes: Some(synth_font()),
        realtime: true,
        stop_after_over: Some(60),
        ..HeadlessOptions::default()
    };
    let mut grants: Vec<(u8, u16, u16)> = Vec::new();
    let end = Instant::now() + Duration::from_secs(120);
    let mut played = false;
    while !played && Instant::now() < end {
        m.update();
        for i in pilot.next(&m) {
            m.input(i);
        }
        if let Some(MenuOutcome::Play {
            server,
            lobby,
            mode,
        }) = m.take_outcome()
        {
            let nick2 = lobby.profile.nick.clone();
            let handoff = dc_client::menu::handoff::LobbyHandoff::new(server, *lobby, mode);
            let mut net = dc_client::NetWorld::start(dc_client::menu::handoff::game_options(
                &base,
                handoff.clone(),
                &nick2,
                nick,
            ));
            let handle = net.handle();
            let mut walk = dc_client::WalkTest::new(dc_client::ViewAngles::default());
            let r = dc_client::run_headless(&mut net, Some(&mut walk), &hopts);
            let st = handle.shutdown(Duration::from_secs(5));
            drop(net);
            println!(
                "match: {:?} grants {:?} end {:?}",
                r.map(|r| r.summary()),
                st.grants,
                st.end
            );
            grants = st.grants.clone();
            let back = GameBack {
                lobby: handoff.take_back(),
                result: handoff.take_result(),
            };
            m.back_from_match(back.lobby, back.result);
            played = true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(m);
    let _ = bot.join();
    let text = std::fs::read_to_string(&log).unwrap_or_default();
    lob.drain();
    if let Some(p) = lob.pid() {
        kill_pid(p);
    }
    let _ = lob.finish(Duration::from_secs(2));
    for p in logged_pids(&text) {
        kill_pid(p);
    }
    assert!(played, "the match was played; lobby log:\n{text}");
    assert!(
        grants.iter().any(|g| g.0 == 0 && u32::from(g.1) == item),
        "the game server granted the bought weapon {item} in slot 0: {grants:?}"
    );
}
