//! nickname-screen (CLI-1480..1484): a login without a character opens the 2009 nickname screen against a real
//! dc-lobby-server started without `--autochar` (TLS on loopback). The scripted player is refused on the client
//! (the login ID, a short name), checks a taken name (taken message, Create stays disabled), checks a free one
//! (free message, Create enabled), picks another portrait (large picture and figure follow, the face value is 6000 +
//! the model's face row) and creates the character: the server list opens the server screen. A second player who
//! checked the same name before is refused at Create (the "name taken" box, Check enabled again). Escape and Cancel
//! ask whether to close the game; yes quits.
//!
//! ROM-gated (skipped without `CA_ROM`: the page, the creation table and the message literals come from the user's
//! ROM). The lobby is stopped by its own PID.
// SPEC: PROTOCOL_2009.md §2.1, §2.2 (0x204 / 0x205, 0x202 / 0x203)
// EVIDENCE: cshell+0x12e550, cshell+0x12cb70, cshell+0x12da20, cshell+0x12e9d0

mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use dc_client::menu::link::LinkOptions;
use dc_client::menu::nickname::{
    models_from_rom, NickArt, CHECK_BUTTON, CMD_CANCEL, CMD_CHECK, CMD_CREATE, CREATE_BUTTON,
    FACE_BASE, MESSAGE, NAME_BOX, NAME_WINDOW, PICTURE, PORTRAIT_AT, PORTRAIT_STEP,
};
use dc_client::menu::sheet::Look;
use dc_client::menu::{Menu, MenuInput, MenuKey, MenuOptions};
use dc_ui_data::WidgetKind;
use support::*;

struct Lobby {
    proc_: Proc,
    log: PathBuf,
    auth: SocketAddr,
}

impl Lobby {
    fn start(tag: &str, watchdog: &Watchdog) -> Lobby {
        let dir = scratch(tag);
        let log = dir.join("lobby.log");
        let (port, lport, cport) = loop {
            let p = (free_tcp_port(), free_tcp_port(), free_tcp_port());
            let mut v = vec![p.0, p.1, p.2, p.1.wrapping_add(1)];
            v.sort();
            v.dedup();
            if v.len() == 4 {
                break p;
            }
        };
        let mut cmd = Command::new(lobby_exe());
        cmd.args([
            "--port",
            &port.to_string(),
            "--lobbyport",
            &lport.to_string(),
        ])
        .args(["--clanport", &cport.to_string(), "--clanweb", "0"])
        .arg("--db")
        .arg(dir.join("lobby.db"))
        .arg("--log")
        .arg(&log)
        .args(["--tls", "loopback"])
        .current_dir(&dir);
        let proc_ = Proc::spawn(cmd);
        watchdog.watch_pid(proc_.pid());
        watchdog.watch_log(&log);
        let up = (0..600).any(|_| {
            std::thread::sleep(Duration::from_millis(50));
            std::net::TcpStream::connect(("127.0.0.1", port)).is_ok()
                && std::net::TcpStream::connect(("127.0.0.1", lport)).is_ok()
        });
        assert!(
            up,
            "lobby did not listen; log:\n{}",
            std::fs::read_to_string(&log).unwrap_or_default()
        );
        Lobby {
            proc_,
            log,
            auth: format!("127.0.0.1:{port}").parse().unwrap(),
        }
    }

    fn stop(mut self) -> String {
        let text = std::fs::read_to_string(&self.log).unwrap_or_default();
        self.proc_.drain();
        if let Some(p) = self.proc_.pid() {
            kill_pid(p);
        }
        let _ = self.proc_.finish(Duration::from_secs(2));
        text
    }
}

fn menu(rom: &std::path::Path, auth: SocketAddr, user: &str, auto: bool) -> Menu {
    let mut link = LinkOptions::new(user);
    link.auto_character = auto;
    Menu::new(MenuOptions {
        auth,
        user: user.into(),
        link,
        rom: Some(rom.to_path_buf()),
        font: None,
        width: 1024,
        height: 768,
    })
}

/// Updates the menu until `ok` holds (10 s at most).
fn until(m: &mut Menu, what: &str, ok: impl Fn(&Menu) -> bool) {
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        m.update();
        if ok(m) {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "{what}: screen {}, box {:?}",
        m.screen_name(),
        m.popup().map(|p| p.text.clone())
    );
}

fn login(m: &mut Menu) {
    m.input(MenuInput::Command(dc_client::menu::login::CMD_LOGIN));
}

fn click_at(m: &mut Menu, at: [f32; 2]) {
    m.input(MenuInput::Move(at));
    m.update();
    m.input(MenuInput::Press(at));
    m.update();
    m.input(MenuInput::Release(at));
    m.update();
}

fn click_button(m: &mut Menu, id: i32) {
    let r = m
        .sheet()
        .part(-1, id, WidgetKind::Button)
        .expect("button")
        .rect();
    click_at(m, [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32]);
}

fn name(m: &Menu) -> String {
    m.sheet()
        .part(NAME_WINDOW, NAME_BOX, WidgetKind::EditBox)
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

fn set_name(m: &mut Menu, s: &str) {
    while !name(m).is_empty() {
        m.input(MenuInput::Key(MenuKey::Backspace));
    }
    for c in s.chars() {
        m.input(MenuInput::Char(c));
    }
    assert_eq!(name(m), s, "the name box takes the typing");
}

fn image_tex(m: &Menu, id: i32) -> String {
    match &m
        .sheet()
        .part(-1, id, WidgetKind::Image)
        .expect("image")
        .look
    {
        Look::Panel { tex, .. } => tex.clone(),
        _ => String::new(),
    }
}

fn enabled(m: &Menu, id: i32) -> bool {
    m.sheet()
        .part(-1, id, WidgetKind::Button)
        .is_some_and(|p| p.enabled)
}

fn to_nickname(m: &mut Menu) {
    login(m);
    until(m, "nickname screen", |m| m.screen_name() == "nickname");
}

#[test]
fn rom_nickname_screen_creates_the_character() {
    let Some(rom) = std::env::var_os("CA_ROM")
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
    else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let art = NickArt::from_rom(Some(&rom));
    let models = models_from_rom(&rom);
    assert!(
        art.free.is_some() && art.taken.is_some() && art.back.is_some(),
        "{art:?}"
    );
    assert!(models.len() >= 4, "creation table: {} models", models.len());
    let watchdog = Watchdog::arm("menu_nickname", Duration::from_secs(120));
    let lobby = Lobby::start("menu-nickname", &watchdog);

    // a character named "holder1" exists (scripted login: made at once, CLI-162)
    let mut holder = menu(&rom, lobby.auth, "holder1", true);
    login(&mut holder);
    until(&mut holder, "holder at the servers", |m| {
        m.screen_name() == "servers"
    });

    // two fresh accounts reach the nickname screen
    let mut a = menu(&rom, lobby.auth, "fresh01", false);
    let mut b = menu(&rom, lobby.auth, "fresh02", false);
    to_nickname(&mut a);
    to_nickname(&mut b);
    assert!(!enabled(&a, CREATE_BUTTON), "Create starts disabled");
    assert!(enabled(&a, CHECK_BUTTON));
    assert_eq!(
        image_tex(&a, PICTURE),
        models[0].large,
        "the first model is chosen"
    );
    let rule = image_tex(&a, MESSAGE);

    // refused on the client: the login ID (any case), a short name; nothing reaches the server
    let sent = a.link_latest_request();
    set_name(&mut a, "FRESH01");
    a.input(MenuInput::Command(CMD_CHECK));
    assert_eq!(Some(image_tex(&a, MESSAGE)), art.account.clone());
    set_name(&mut a, "abc");
    a.input(MenuInput::Command(CMD_CHECK));
    assert_eq!(Some(image_tex(&a, MESSAGE)), art.length.clone());
    assert_ne!(image_tex(&a, MESSAGE), rule);
    assert_eq!(a.link_latest_request(), sent, "no check went out");

    // a taken name: the taken message, Create stays disabled
    set_name(&mut a, "holder1");
    click_button(&mut a, CHECK_BUTTON);
    until(&mut a, "taken", |m| {
        Some(image_tex(m, MESSAGE)) == art.taken.clone()
    });
    assert!(!enabled(&a, CREATE_BUTTON));
    // Create before a successful check: the check-first message
    a.input(MenuInput::Command(CMD_CREATE));
    assert_eq!(Some(image_tex(&a, MESSAGE)), art.check_first.clone());

    // a free name (both players check it): the free message, Create enabled
    for m in [&mut a, &mut b] {
        set_name(m, "NewHero");
        m.input(MenuInput::Key(MenuKey::Enter));
        until(m, "free", |m| {
            Some(image_tex(m, MESSAGE)) == art.free.clone()
        });
        assert!(enabled(m, CREATE_BUTTON));
    }

    // another portrait: its large picture, the figure's face, the face value of the creation
    let pick = 3;
    let at = [
        (PORTRAIT_AT[0] + pick as i32 * PORTRAIT_STEP + models[pick].small_size[0] / 2) as f32,
        (PORTRAIT_AT[1] + models[pick].small_size[1] / 2) as f32,
    ];
    click_at(&mut a, at);
    assert_eq!(image_tex(&a, PICTURE), models[pick].large);
    let req = a
        .figure_request()
        .expect("the chosen model stands on the screen");
    assert!(req.look.is_some(), "the figure has a body");
    // DC_MENU_SHOTS: the composed screen as a PNG (ROM-derived: a local file, never committed)
    if let Some(dir) = std::env::var_os("DC_MENU_SHOTS") {
        let img = a.image();
        let _ = std::fs::create_dir_all(&dir);
        let p = PathBuf::from(dir).join("nickname.png");
        let _ = image::save_buffer(
            &p,
            &img.rgba,
            img.width,
            img.height,
            image::ColorType::Rgba8,
        );
    }

    click_button(&mut a, CREATE_BUTTON);
    assert_eq!(
        a.link_latest_request(),
        format!(
            "CreateCharacter({}, \"NewHero\")",
            FACE_BASE + models[pick].face
        )
    );
    assert!(
        !enabled(&a, CHECK_BUTTON) && !enabled(&a, CREATE_BUTTON),
        "both wait for the answer"
    );
    until(&mut a, "server screen after the creation", |m| {
        m.screen_name() == "servers"
    });

    // the second player's Create is refused now: the "name taken" box, Check enabled again
    click_button(&mut b, CREATE_BUTTON);
    let taken = b.cat.text(dc_client::menu::nickname::TEXT_TAKEN);
    until(&mut b, "refusal box", |m| {
        m.popup().is_some_and(|p| p.text == taken)
    });
    assert!(enabled(&b, CHECK_BUTTON) && !enabled(&b, CREATE_BUTTON));
    assert_eq!(b.screen_name(), "nickname");

    // Escape asks to close the game (no keeps the screen); Cancel asks the same, yes quits
    let mut c = menu(&rom, lobby.auth, "fresh03", false);
    to_nickname(&mut c);
    c.input(MenuInput::Key(MenuKey::Escape));
    let ask = c.cat.text(dc_client::menu::popup::text_ids::ASK_QUIT);
    assert_eq!(c.popup().map(|p| p.text.clone()), Some(ask.clone()));
    let no = c.popup().unwrap().buttons()[1];
    click_at(
        &mut c,
        [(no[0] + no[2] / 2) as f32, (no[1] + no[3] / 2) as f32],
    );
    assert!(c.popup().is_none() && !c.quitting());
    assert_eq!(c.screen_name(), "nickname");
    c.input(MenuInput::Command(CMD_CANCEL));
    assert_eq!(c.popup().map(|p| p.text.clone()), Some(ask));
    let yes = c.popup().unwrap().buttons()[0];
    click_at(
        &mut c,
        [(yes[0] + yes[2] / 2) as f32, (yes[1] + yes[3] / 2) as f32],
    );
    assert!(c.quitting(), "yes closes the game");

    // the created character logs in straight to the server list now
    let mut again = menu(&rom, lobby.auth, "fresh01", false);
    login(&mut again);
    until(&mut again, "servers on the next login", |m| {
        m.screen_name() == "servers"
    });

    let text = lobby.stop();
    assert!(
        text.contains("fresh01") && text.contains("has no character"),
        "{text}"
    );
}
