//! client-register (CLI-1450 / CLI-1451): the login screen's register button opens the in-client account creation
//! dialog. The button asks the lobby's web port whether it registers (`GET /register` as JSON, LG-210), the dialog
//! takes ID, password twice and the invite code (Tab order fields, Create, Cancel), Create posts the registration
//! form and the answer shows in the message box: a refusal keeps the dialog, success closes it and fills the ID box.
//!
//! login-recovery: the find-ID / find-password buttons open the same kind of dialog (`GET /recover`, then the
//! `/recover/...` posts) and the register dialog takes an email address when the lobby asks for one.
//!
//! ROM-gated (skipped without `CA_ROM`: the login layout and its button come from the user's ROM). A fake web port
//! on a local socket answers in place of the lobby (`DC_CLANWEB` points the client at it).
// DISASM: cshell+0x11d4e0 (command 0x2712 = 10002 opened the passport Registration/Signup page)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::login::{CMD_REGISTER, ID_BOX, PASSWORD_BOX};
use dc_client::menu::signup;
use dc_client::menu::{Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::steps::{resolve, Resolved};
use dc_ui_data::WidgetKind;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// `DC_CLANWEB` is one value per process: the tests take turns.
static WEB_ENV: Mutex<()> = Mutex::new(());

fn menu() -> Option<Menu> {
    menu_sized(1024, 768)
}

/// The login screen in a `w` x `h` pixel window (the menu works in virtual 1024 x 768 pixels either way).
fn menu_sized(w: u32, h: u32) -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: String::new(),
        link: LinkOptions::new("register"),
        rom: Some(rom),
        font: None,
        width: w,
        height: h,
    }))
}

/// A user's click: move, a frame, press, a frame, release, then some frames.
fn real_click(m: &mut Menu, at: [f32; 2]) {
    m.input(MenuInput::Move(at));
    m.update();
    m.input(MenuInput::Press(at));
    m.update();
    m.input(MenuInput::Release(at));
    for _ in 0..5 {
        m.update();
        let _ = m.items();
    }
}

fn part_at(m: &Menu, name: &str) -> [f32; 2] {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}");
    };
    at
}

/// The dialog's title as drawn (virtual pixels), if it is drawn.
fn title_at(m: &mut Menu) -> Option<[f32; 2]> {
    m.items().iter().find_map(|i| match i {
        DrawItem::Text { text, pos, .. } | DrawItem::Label { text, pos, .. }
            if text == signup::T_TITLE =>
        {
            Some(*pos)
        }
        _ => None,
    })
}

/// The open box's first button, clicked with the mouse.
fn click_box_ok(m: &mut Menu) {
    let r = m.popup().expect("a box").buttons()[0];
    real_click(m, [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32]);
    assert!(m.popup().is_none(), "the box closes on its OK button");
}

/// The login screen takes clicks: a click in the ID box gives it the keyboard.
fn login_takes_clicks(m: &mut Menu) {
    let at = part_at(m, "login.password");
    real_click(m, at);
    let focused = m.sheet().focused().map(|i| m.sheet().parts[i].id);
    assert_eq!(
        focused,
        Some(PASSWORD_BOX),
        "the login screen has the input"
    );
    let at = part_at(m, "login.id");
    real_click(m, at);
    let focused = m.sheet().focused().map(|i| m.sheet().parts[i].id);
    assert_eq!(focused, Some(ID_BOX), "the login screen has the input");
}

fn click(m: &mut Menu, name: &str) {
    let Resolved::Part { at, .. } = resolve(m, name) else {
        panic!("{name}");
    };
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

fn texts(m: &mut Menu) -> Vec<String> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn type_text(m: &mut Menu, s: &str) {
    for c in s.chars() {
        m.input(MenuInput::Char(c));
    }
}

/// Updates the menu until a box other than the wait box is up.
fn box_text(m: &mut Menu) -> String {
    let wait = m.cat.text(dc_client::menu::popup::text_ids::CONNECTING);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        m.update();
        if let Some(p) = m.popup().filter(|p| p.text != wait) {
            return p.text.clone();
        }
        if m.popup().is_none() && dialog_up(m) {
            return String::new();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no answer");
}

fn dialog_up(m: &mut Menu) -> bool {
    texts(m).iter().any(|t| t == signup::T_TITLE)
}

fn edit_text(m: &Menu, id: i32) -> Option<String> {
    m.sheet()
        .part(0, id, WidgetKind::EditBox)
        .map(|p| p.text.clone())
}

/// A fake web port: answers `GET /register` with `info` and each post with the next of `posts`; keeps the requests.
fn fake_lobby(info: &'static str, posts: Vec<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut posts = posts.into_iter();
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let req = loop {
                let n = s.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                // CLI-1470: the client tries TLS first; this plain fake closes the handshake, the client goes plain
                if buf.first() == Some(&0x16) {
                    break String::new();
                }
                let t = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = t.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= len || n == 0 {
                        break t;
                    }
                }
                if n == 0 {
                    break t;
                }
            };
            if req.is_empty() {
                continue;
            }
            let answer = if req.starts_with("GET") {
                info
            } else {
                posts.next().unwrap_or("{}")
            };
            keep.lock().unwrap().push(req);
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    answer.len(),
                    answer
                )
                .as_bytes(),
            );
        }
    });
    (addr, seen)
}

#[test]
fn rom_register_button_opens_the_dialog_and_posts_the_form() {
    let _env = WEB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    assert_eq!(m.screen_name(), "login");
    // the register button is one of the layout's web buttons (controls 4..8) with command 10002
    let reg = m
        .sheet()
        .parts
        .iter()
        .find(|p| p.kind == WidgetKind::Button && p.command == CMD_REGISTER)
        .map(|p| p.id);
    assert!(
        reg.is_some_and(|id| (4..=8).contains(&id)),
        "register control {reg:?}"
    );

    // nobody answers on the web port: the "server not reachable" box
    let closed = TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = closed.local_addr().unwrap().to_string();
    drop(closed);
    std::env::set_var("DC_CLANWEB", &dead);
    click(&mut m, "login.register");
    assert_eq!(m.link_latest_request(), format!("RegisterInfo({dead})"));
    assert_eq!(box_text(&mut m), signup::T_UNREACHABLE);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!dialog_up(&mut m));

    // registration off: its box, no dialog
    let (off, _) = fake_lobby(r#"{"open":false,"invite":false}"#, vec![]);
    std::env::set_var("DC_CLANWEB", &off);
    click(&mut m, "login.register");
    assert_eq!(box_text(&mut m), signup::T_OFF);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!dialog_up(&mut m));

    // on, with invite codes: the dialog with the code row
    let (web, seen) = fake_lobby(
        r#"{"open":true,"invite":true}"#,
        vec![
            r#"{"ok":false,"message":"Cannot register: that name is taken."}"#,
            r#"{"ok":true,"login":"Neo"}"#,
        ],
    );
    std::env::set_var("DC_CLANWEB", &web);
    click(&mut m, "login.register");
    assert_eq!(box_text(&mut m), "");
    assert!(dialog_up(&mut m));
    let all = texts(&mut m);
    for want in signup::T_LABELS {
        assert!(all.iter().any(|t| t == want), "{want}: {all:?}");
    }
    if let Some(dir) = std::env::var_os("DC_REGISTER_SHOT") {
        type_text(&mut m, "Neo");
        let i = m.image();
        let f = dc_render::FrameImage {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
        };
        let _ = f.save_png(&std::path::Path::new(&dir).join("register_dialog.png"));
        for _ in 0..3 {
            m.input(MenuInput::Key(MenuKey::Backspace));
        }
    }
    // Tab order: ID, password, password again, code, then Create
    type_text(&mut m, "Neo");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "pa&s w1");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "pa&s w1");
    m.input(MenuInput::Key(MenuKey::Tab));
    // CLI-1510: the invite code comes from the clipboard (the line break is dropped)
    m.input(MenuInput::Paste("ABCD
 1234".to_string()));
    m.input(MenuInput::Key(MenuKey::Enter));
    let sent = m.link_latest_request();
    assert_eq!(
        sent,
        format!("Register({web}, RegisterForm(Neo, code set, passwords hidden))")
    );
    assert!(!sent.contains("pa&s"), "the password is never shown");
    // the refusal: the lobby's reason in the box, the dialog stays with its fields
    assert_eq!(box_text(&mut m), "Cannot register: that name is taken.");
    {
        let reqs = seen.lock().unwrap();
        let post = reqs.iter().find(|r| r.starts_with("POST")).expect("a post");
        assert!(post.starts_with("POST /register HTTP/1.1\r\n"), "{post}");
        assert!(post.contains("\r\nAccept: application/json\r\n"), "{post}");
        assert!(
            post.ends_with("\r\n\r\nname=Neo&pw=pa%26s%20w1&pw2=pa%26s%20w1&code=ABCD%201234"),
            "{post}"
        );
    }
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(dialog_up(&mut m), "a refusal keeps the dialog");
    // Create again: success closes the dialog, fills the ID box and puts the keyboard in the password box
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(box_text(&mut m), signup::ready_message("Neo"));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!dialog_up(&mut m));
    assert_eq!(edit_text(&m, ID_BOX).as_deref(), Some("Neo"));
    assert_eq!(edit_text(&m, PASSWORD_BOX).as_deref(), Some(""));
    let focused = m.sheet().focused();
    assert_eq!(
        focused.map(|i| m.sheet().parts[i].id),
        Some(PASSWORD_BOX),
        "the password box has the keyboard"
    );
    // Escape cancels a reopened dialog
    click(&mut m, "login.register");
    assert_eq!(box_text(&mut m), "");
    assert!(dialog_up(&mut m));
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(!dialog_up(&mut m));
}

/// register-dialog-fix (user, 2026-10-30): in a 2560 x 1440 window the register button "flashed the register screen
/// but then it disappeared and couldn't click anything". The dialog was centred in window pixels (x 1041 of the
/// 1024 wide virtual screen: off screen) and kept every input. A real click sequence at both sizes: the dialog is on
/// the screen and stays through clicks until Cancel / Escape, the boxes close with their OK button, and the login
/// screen takes clicks afterwards.
#[test]
fn rom_register_dialog_stays_up_and_gives_the_input_back_at_any_window_size() {
    let _env = WEB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    for (w, h) in [(1024, 768), (2560, 1440)] {
        let Some(mut m) = menu_sized(w, h) else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        let reg = part_at(&mut m, "login.register");

        // unreachable and off: the box closes with a click and the screen takes clicks again
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = closed.local_addr().unwrap().to_string();
        drop(closed);
        std::env::set_var("DC_CLANWEB", &dead);
        real_click(&mut m, reg);
        assert_eq!(box_text(&mut m), signup::T_UNREACHABLE, "{w}x{h}");
        click_box_ok(&mut m);
        login_takes_clicks(&mut m);
        let (off, _) = fake_lobby(r#"{"open":false,"invite":false}"#, vec![]);
        std::env::set_var("DC_CLANWEB", &off);
        real_click(&mut m, reg);
        assert_eq!(box_text(&mut m), signup::T_OFF, "{w}x{h}");
        click_box_ok(&mut m);
        assert!(title_at(&mut m).is_none());
        login_takes_clicks(&mut m);

        // open: the dialog is drawn on the virtual screen and stays through frames and clicks
        let (web, _) = fake_lobby(r#"{"open":true,"invite":false}"#, vec![]);
        std::env::set_var("DC_CLANWEB", &web);
        real_click(&mut m, reg);
        assert_eq!(box_text(&mut m), "", "{w}x{h}");
        let want = signup::SignupBox::centred(false);
        let t = title_at(&mut m).expect("the dialog is drawn");
        assert!(
            t[0] >= 0.0 && t[0] < 1024.0 && t[1] >= 0.0 && t[1] < 768.0,
            "{w}x{h}: title at {t:?}"
        );
        assert_eq!(
            [t[0] as i32, t[1] as i32],
            [
                want.at[0] + signup::TITLE_AT[0],
                want.at[1] + signup::TITLE_AT[1]
            ]
        );
        for _ in 0..30 {
            m.update();
        }
        // a click in the password field and on the empty window area keep it up; the press on Register again too
        let f = want.field_rect(1);
        real_click(&mut m, [(f[0] + 5) as f32, (f[1] + 5) as f32]);
        real_click(&mut m, [(want.at[0] + 5) as f32, (want.at[1] + 30) as f32]);
        real_click(&mut m, reg);
        type_text(&mut m, "abc");
        assert!(title_at(&mut m).is_some(), "{w}x{h}: the dialog stays up");
        assert!(m.popup().is_none());
        // Cancel closes it and the login screen takes clicks
        let c = want.cancel_at();
        real_click(&mut m, [(c[0] + 10) as f32, (c[1] + 10) as f32]);
        assert!(title_at(&mut m).is_none(), "{w}x{h}: Cancel closes it");
        login_takes_clicks(&mut m);
        // reopened: Escape closes it and the login screen takes clicks
        real_click(&mut m, reg);
        assert_eq!(box_text(&mut m), "");
        assert!(title_at(&mut m).is_some());
        m.input(MenuInput::Key(MenuKey::Escape));
        assert!(title_at(&mut m).is_none(), "{w}x{h}: Escape closes it");
        login_takes_clicks(&mut m);
    }
}

/// The dialog's button: its picture (name, rectangle) and its caption's centre, as drawn at `at`.
fn button_drawn(m: &mut Menu, at: [i32; 2], caption: &str) -> (String, [f32; 4], [f32; 2]) {
    let items = m.items();
    let img = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Image { tex, dst, .. }
                if dst[0] as i32 == at[0] && dst[1] as i32 == at[1] =>
            {
                Some((tex.clone(), *dst))
            }
            _ => None,
        })
        .expect("the button picture");
    let label = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Label { text, pos, .. } | DrawItem::Text { text, pos, .. } if text == caption => {
                Some(*pos)
            }
            _ => None,
        })
        .expect("the caption");
    (img.0, img.1, label)
}

#[test]
fn rom_register_dialog_buttons_align_hover_and_sound_like_the_message_boxes() {
    let _env = WEB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let reg = part_at(&mut m, "login.register");
    let (web, _) = fake_lobby(r#"{"open":true,"invite":false}"#, vec![]);
    std::env::set_var("DC_CLANWEB", &web);
    real_click(&mut m, reg);
    assert_eq!(box_text(&mut m), "");
    let _ = m.take_sound_cues();
    let d = signup::SignupBox::centred(false);
    let cancel_text = m.cat.text(dc_client::menu::usebox::CANCEL_TEXT);

    // the captions lie inside their buttons' pictures, centred within two pixels
    for (at, text) in [
        (d.create_at(), signup::T_CREATE.to_string()),
        (d.cancel_at(), cancel_text.clone()),
    ] {
        let (_tex, dst, c) = button_drawn(&mut m, at, &text);
        assert!(c[0] > dst[0] && c[0] < dst[0] + dst[2], "{text}: {c:?} in {dst:?}");
        assert!(c[1] > dst[1] && c[1] < dst[1] + dst[3], "{text}: {c:?} in {dst:?}");
        // the picture itself is 100 px wide in its 120 px control (popup::CAPTION_AT, cshell+0x26f3c0)
        assert!((c[0] - (dst[0] + 50.0)).abs() <= 2.0, "{text} x {c:?} {dst:?}");
        assert!((c[1] - (dst[1] + dst[3] / 2.0)).abs() <= 2.0, "{text} y {c:?} {dst:?}");
    }

    // hover and pressed looks: the picture changes with the pointer, as the message boxes' buttons do
    let c = d.cancel_at();
    let on = [(c[0] + 20) as f32, (c[1] + 10) as f32];
    let focus_look = button_drawn(&mut m, c, &cancel_text).0;
    let rest = button_drawn(&mut m, d.create_at(), signup::T_CREATE).0;
    // the first stop is a field: no button is lit
    assert_eq!(rest, button_drawn(&mut m, d.create_at(), signup::T_CREATE).0);
    m.input(MenuInput::Move(on));
    assert_eq!(m.pending_sounds(), [dc_client::menu::frontfx::SOUND_SELECT_CHANGE]);
    let hover = button_drawn(&mut m, c, &cancel_text).0;
    assert_ne!(hover, focus_look, "hover look differs from the normal look");
    if let Some(dir) = std::env::var_os("DC_REGISTER_SHOT") {
        let i = m.image();
        let f = dc_render::FrameImage {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
        };
        let _ = f.save_png(&std::path::Path::new(&dir).join("signup_after.png"));
    }
    m.input(MenuInput::Move([on[0] + 1.0, on[1]]));
    assert_eq!(m.pending_sounds().len(), 1, "one sound per button");
    m.input(MenuInput::Press(on));
    let down = button_drawn(&mut m, c, &cancel_text).0;
    assert_ne!(down, hover, "the pressed look differs from hover");
    assert_eq!(m.pending_sounds().len(), 1, "no sound on press");
    // release on the button: the select click and the dialog closes
    m.input(MenuInput::Release(on));
    assert_eq!(
        m.pending_sounds(),
        [
            dc_client::menu::frontfx::SOUND_SELECT_CHANGE,
            dc_client::menu::frontfx::SOUND_SELECT
        ]
    );
    assert!(title_at(&mut m).is_none(), "Cancel closes the dialog");

    // the Register button itself: select change on hover, select on release
    let _ = m.take_sound_cues();
    m.input(MenuInput::Move(reg));
    m.input(MenuInput::Press(reg));
    m.input(MenuInput::Release(reg));
    let s = m.pending_sounds().to_vec();
    assert_eq!(
        s,
        [
            dc_client::menu::frontfx::SOUND_SELECT_CHANGE,
            dc_client::menu::frontfx::SOUND_SELECT
        ]
    );

    // pressed on a button, released elsewhere: no click; Escape closes with the select sound
    assert_eq!(box_text(&mut m), "");
    let _ = m.take_sound_cues();
    m.input(MenuInput::Press(on));
    m.input(MenuInput::Release([5.0, 5.0]));
    assert!(title_at(&mut m).is_some(), "no click when released elsewhere");
    assert!(!m.pending_sounds().contains(&dc_client::menu::frontfx::SOUND_SELECT));
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(m.pending_sounds().contains(&dc_client::menu::frontfx::SOUND_SELECT));
    assert!(title_at(&mut m).is_none());
}

// ---- login-recovery (CLI-1500 .. CLI-1503) ----

/// A fake web port answering by path: `gets` = (path, status, body) for GET requests, `posts` = the answers to the
/// posts in order (any path); keeps the requests.
fn fake_web(
    gets: Vec<(&'static str, u16, &'static str)>,
    posts: Vec<&'static str>,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let keep = Arc::clone(&seen);
    std::thread::spawn(move || {
        let mut posts = posts.into_iter();
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            let req = loop {
                let n = s.read(&mut chunk).unwrap_or(0);
                buf.extend_from_slice(&chunk[..n]);
                // CLI-1470: the client tries TLS first; this plain fake closes the handshake, the client goes plain
                if buf.first() == Some(&0x16) {
                    break String::new();
                }
                let t = String::from_utf8_lossy(&buf).to_string();
                if let Some((head, body)) = t.split_once("\r\n\r\n") {
                    let len = head
                        .lines()
                        .find_map(|l| l.strip_prefix("Content-Length: "))
                        .and_then(|v| v.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if body.len() >= len || n == 0 {
                        break t;
                    }
                }
                if n == 0 {
                    break t;
                }
            };
            if req.is_empty() {
                continue;
            }
            let path = req.split_whitespace().nth(1).unwrap_or_default().to_string();
            let (status, answer) = if req.starts_with("GET") {
                gets.iter()
                    .find(|g| g.0 == path)
                    .map_or((404, "Not found"), |g| (g.1, g.2))
            } else {
                (200, posts.next().unwrap_or("{}"))
            };
            keep.lock().unwrap().push(req);
            let _ = s.write_all(
                format!(
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    answer.len(),
                    answer
                )
                .as_bytes(),
            );
        }
    });
    (addr, seen)
}

/// Updates the menu until a box other than the wait box is up (its text) or the dialog titled `title` is up ("").
fn answer_or_dialog(m: &mut Menu, title: &str) -> String {
    let wait = m.cat.text(dc_client::menu::popup::text_ids::CONNECTING);
    let end = Instant::now() + Duration::from_secs(10);
    while Instant::now() < end {
        m.update();
        if let Some(p) = m.popup().filter(|p| p.text != wait) {
            return p.text.clone();
        }
        if m.popup().is_none() && texts(m).iter().any(|t| t == title) {
            return String::new();
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("no answer");
}

fn shot(m: &mut Menu, name: &str) {
    if let Some(dir) = std::env::var_os("DC_REGISTER_SHOT") {
        let i = m.image();
        let f = dc_render::FrameImage {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
        };
        let _ = f.save_png(&std::path::Path::new(&dir).join(name));
    }
}

/// The last post the fake web port took.
fn last_post(seen: &Arc<Mutex<Vec<String>>>) -> String {
    seen.lock()
        .unwrap()
        .iter()
        .rev()
        .find(|r| r.starts_with("POST"))
        .cloned()
        .expect("a post")
}

fn posts(seen: &Arc<Mutex<Vec<String>>>) -> usize {
    seen.lock().unwrap().iter().filter(|r| r.starts_with("POST")).count()
}

/// A click on the text link of a web button: the top of its control (the ROM layout gives the find-password control
/// a 74 x 68 rectangle whose middle lies on the quit button; its caption is the top 15 px row, as find ID's).
fn click_link(m: &mut Menu, name: &str) {
    let Resolved::Part { part, .. } = resolve(m, name) else {
        panic!("{name}");
    };
    let r = m.sheet().parts[part].rect();
    let at = [(r[0] + r[2] / 2) as f32, (r[1] + 7) as f32];
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
}

/// CLI-1500: a lobby whose `GET /register` says `"email": true` gets the email row; the client refuses an address
/// without one @ and a dot after it; the address goes in the form as `email`. A lobby without the key: no row.
#[test]
fn rom_register_dialog_email_row_follows_the_lobby() {
    let _env = WEB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let (old, _) = fake_lobby(r#"{"open":true,"invite":false}"#, vec![]);
    std::env::set_var("DC_CLANWEB", &old);
    click(&mut m, "login.register");
    assert_eq!(box_text(&mut m), "");
    assert!(!texts(&mut m).iter().any(|t| t == signup::T_EMAIL), "no email row");
    m.input(MenuInput::Key(MenuKey::Escape));

    let (web, seen) = fake_lobby(
        r#"{"open":true,"invite":false,"email":true}"#,
        vec![r#"{"ok":false,"message":"That address is not valid."}"#],
    );
    std::env::set_var("DC_CLANWEB", &web);
    click(&mut m, "login.register");
    assert_eq!(box_text(&mut m), "");
    assert!(texts(&mut m).iter().any(|t| t == signup::T_EMAIL), "the email row");
    // Tab order: ID, password, again, email, Create
    type_text(&mut m, "Neo");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "secret12");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "secret12");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "neo@mail");
    shot(&mut m, "register_email.png");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("RegisterInfo({web})"), "no post");
    assert!(texts(&mut m).iter().any(|t| t == signup::T_BAD_EMAIL));
    type_text(&mut m, ".example");
    m.input(MenuInput::Key(MenuKey::Tab));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        format!("Register({web}, RegisterForm(Neo, code none, email set, passwords hidden))")
    );
    // the server's message wins
    assert_eq!(box_text(&mut m), "That address is not valid.");
    let post = last_post(&seen);
    assert!(post.starts_with("POST /register HTTP/1.1\r\n"), "{post}");
    assert!(
        post.ends_with("\r\n\r\nname=Neo&pw=secret12&pw2=secret12&code=&email=neo%40mail.example"),
        "{post}"
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(!dialog_up(&mut m));
}

/// CLI-1501 .. CLI-1503: find ID and find password. `GET /recover` first: unreachable -> the unreachable box, an old
/// lobby (404) or `"mail": false` -> the not-set-up box, `"mail": true` -> the dialog. The exact posts of the three
/// requests, the code step after a sent code, Back, a refused code, the changed password, Tab order and Escape.
#[test]
fn rom_find_id_and_find_password_dialogs() {
    let _env = WEB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    for name in ["login.find_id", "login.find_pw"] {
        // nobody answers
        let closed = TcpListener::bind("127.0.0.1:0").unwrap();
        let dead = closed.local_addr().unwrap().to_string();
        drop(closed);
        std::env::set_var("DC_CLANWEB", &dead);
        click_link(&mut m, name);
        assert_eq!(m.link_latest_request(), format!("RecoverInfo({dead})"));
        assert_eq!(answer_or_dialog(&mut m, "-"), signup::T_UNREACHABLE, "{name}");
        m.input(MenuInput::Key(MenuKey::Enter));
        // an old lobby: no /recover (404)
        let (old, _) = fake_web(vec![], vec![]);
        std::env::set_var("DC_CLANWEB", &old);
        click_link(&mut m, name);
        assert_eq!(answer_or_dialog(&mut m, "-"), signup::T_RECOVER_OFF, "{name}");
        m.input(MenuInput::Key(MenuKey::Enter));
        // mail off
        let (off, _) = fake_web(vec![("/recover", 200, r#"{"mail":false}"#)], vec![]);
        std::env::set_var("DC_CLANWEB", &off);
        click_link(&mut m, name);
        assert_eq!(answer_or_dialog(&mut m, "-"), signup::T_RECOVER_OFF, "{name}");
        m.input(MenuInput::Key(MenuKey::Enter));
        assert!(m.popup().is_none());
        assert!(!texts(&mut m)
            .iter()
            .any(|t| t == signup::T_FIND_ID || t == signup::T_FIND_PASSWORD));
    }

    // find ID
    let sent = "If that address has accounts, a mail with their IDs is on its way.";
    let (web, seen) = fake_web(
        vec![("/recover", 200, r#"{"mail":true}"#)],
        vec![
            r#"{"ok":true,"message":"If that address has accounts, a mail with their IDs is on its way."}"#,
            r#"{"ok":true,"message":"If the ID and address match, a code is on its way."}"#,
            r#"{"ok":true,"message":"If the ID and address match, a code is on its way."}"#,
            r#"{"ok":false,"message":"Wrong or expired code."}"#,
            r#"{"ok":true,"message":"Password changed."}"#,
        ],
    );
    std::env::set_var("DC_CLANWEB", &web);
    click_link(&mut m, "login.find_id");
    assert_eq!(answer_or_dialog(&mut m, signup::T_FIND_ID), "");
    let all = texts(&mut m);
    assert!(all.iter().any(|t| t == signup::T_EMAIL) && all.iter().any(|t| t == signup::T_SEND));
    // Tab order: email, Send, Cancel, email: Enter on Cancel closes
    m.input(MenuInput::Key(MenuKey::Tab));
    m.input(MenuInput::Key(MenuKey::Tab));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!texts(&mut m).iter().any(|t| t == signup::T_FIND_ID), "Cancel by Tab + Enter");
    click_link(&mut m, "login.find_id");
    assert_eq!(answer_or_dialog(&mut m, signup::T_FIND_ID), "");
    type_text(&mut m, "neo@mail.example");
    shot(&mut m, "find_id.png");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_latest_request(), format!("Recover({web}, FindId(email hidden))"));
    assert_eq!(answer_or_dialog(&mut m, "-"), sent);
    let post = last_post(&seen);
    assert!(post.starts_with("POST /recover/id HTTP/1.1\r\n"), "{post}");
    assert!(post.contains("\r\nAccept: application/json\r\n"), "{post}");
    assert!(post.ends_with("\r\n\r\nemail=neo%40mail.example"), "{post}");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_none());
    assert!(!texts(&mut m).iter().any(|t| t == signup::T_FIND_ID), "OK closes");

    // find password, step 1: login ID + email, Send code
    click_link(&mut m, "login.find_pw");
    assert_eq!(answer_or_dialog(&mut m, signup::T_FIND_PASSWORD), "");
    let all = texts(&mut m);
    for want in [signup::T_LOGIN_ID, signup::T_EMAIL, signup::T_SEND_CODE] {
        assert!(all.iter().any(|t| t == want), "{want}: {all:?}");
    }
    // an empty ID and a bad address post nothing
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(texts(&mut m).iter().any(|t| t == signup::T_ENTER_LOGIN));
    type_text(&mut m, "Neo");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "neo-at-mail");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(texts(&mut m).iter().any(|t| t == signup::T_BAD_EMAIL));
    assert_eq!(posts(&seen), 1);
    for _ in 0..11 {
        m.input(MenuInput::Key(MenuKey::Backspace));
    }
    type_text(&mut m, "neo@mail.example");
    shot(&mut m, "find_password.png");
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        m.link_latest_request(),
        format!("Recover({web}, PasswordCode(Neo, email hidden))")
    );
    assert_eq!(
        answer_or_dialog(&mut m, "-"),
        "If the ID and address match, a code is on its way."
    );
    let post = last_post(&seen);
    assert!(post.starts_with("POST /recover/password HTTP/1.1\r\n"), "{post}");
    assert!(post.ends_with("\r\n\r\nname=Neo&email=neo%40mail.example"), "{post}");
    m.input(MenuInput::Key(MenuKey::Enter));
    // step 2: code, new password, again; Change, Back, Cancel
    let all = texts(&mut m);
    for want in [signup::T_CODE, signup::T_NEW_PASSWORD, signup::T_CHANGE, signup::T_BACK] {
        assert!(all.iter().any(|t| t == want), "{want}: {all:?}");
    }
    // Back (fields, Change, Back: four Tabs) returns to step 1 with its fields; Send code again
    for _ in 0..4 {
        m.input(MenuInput::Key(MenuKey::Tab));
    }
    m.input(MenuInput::Key(MenuKey::Enter));
    let all = texts(&mut m);
    assert!(all.iter().any(|t| t == signup::T_SEND_CODE), "back at step 1");
    assert!(all.iter().any(|t| t == "Neo") && all.iter().any(|t| t == "neo@mail.example"));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        answer_or_dialog(&mut m, "-"),
        "If the ID and address match, a code is on its way."
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    type_text(&mut m, "K7Q2M9");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "n&w pass12");
    m.input(MenuInput::Key(MenuKey::Tab));
    type_text(&mut m, "n&w pass12");
    shot(&mut m, "find_password_code.png");
    m.input(MenuInput::Key(MenuKey::Enter));
    let req = m.link_latest_request();
    assert_eq!(
        req,
        format!("Recover({web}, PasswordChange(Neo, code and passwords hidden))")
    );
    // a refused code: the lobby's message, the dialog stays at step 2
    assert_eq!(answer_or_dialog(&mut m, "-"), "Wrong or expired code.");
    let post = last_post(&seen);
    assert!(post.starts_with("POST /recover/password/confirm HTTP/1.1\r\n"), "{post}");
    assert!(
        post.ends_with("\r\n\r\nname=Neo&code=K7Q2M9&pw=n%26w%20pass12&pw2=n%26w%20pass12"),
        "{post}"
    );
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(texts(&mut m).iter().any(|t| t == signup::T_CHANGE), "a refusal keeps step 2");
    // Change again: ok closes the dialog, fills the ID box, focuses the password box
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(answer_or_dialog(&mut m, "-"), signup::changed_message("Neo"));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(!texts(&mut m).iter().any(|t| t == signup::T_FIND_PASSWORD));
    assert_eq!(edit_text(&m, ID_BOX).as_deref(), Some("Neo"));
    assert_eq!(edit_text(&m, PASSWORD_BOX).as_deref(), Some(""));
    let focused = m.sheet().focused();
    assert_eq!(focused.map(|i| m.sheet().parts[i].id), Some(PASSWORD_BOX));
    assert_eq!(posts(&seen), 5);

    // Escape closes a reopened dialog and the login screen takes clicks
    click_link(&mut m, "login.find_pw");
    assert_eq!(answer_or_dialog(&mut m, signup::T_FIND_PASSWORD), "");
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(!texts(&mut m).iter().any(|t| t == signup::T_FIND_PASSWORD));
    login_takes_clicks(&mut m);
}


/// signup-frame: with the invite row and the email row (five rows) the window is taller and the frame art grows with it.
#[test]
fn rom_register_dialog_five_rows_keep_buttons_inside_the_frame() {
    let _env = WEB_ENV.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let (web, _) = fake_lobby(r#"{"open":true,"invite":true,"email":true}"#, vec![]);
    std::env::set_var("DC_CLANWEB", &web);
    click(&mut m, "login.register");
    assert_eq!(box_text(&mut m), "");
    let all = texts(&mut m);
    for want in [signup::T_EMAIL, signup::T_CREATE] {
        assert!(all.iter().any(|t| t == want), "{want}: {all:?}");
    }
    type_text(&mut m, "Neo");
    shot(&mut m, "register_five_rows.png");
}
