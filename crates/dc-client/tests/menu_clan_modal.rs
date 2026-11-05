//! lobby20 item 1: the clan screen's web window (window 5) is modal and drawn last: a long clan chat line does not
//! draw over the open page, and the buttons behind it take no clicks.
//!
//! ROM-gated (skipped without `CA_ROM`), no lobby.

use dc_client::hud::items::DrawItem;
use dc_client::menu::clan::{CMD_LEAVE, CMD_NOTICE, CMD_WEB_CLOSE, NOTICE_WINDOW, WEB_WINDOW};
use dc_client::menu::link::{ClanInfoView, LinkOptions, CLAN_ROLE_LEADER};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_ui_data::WidgetKind;

fn menu() -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.open(Go::Lobby);
    m.data.char_extras.clan_id = 7;
    m.data.char_extras.clan_role = CLAN_ROLE_LEADER;
    m.data.clan.info = Some(ClanInfoView {
        number: 7,
        name: "Testers".into(),
        ..ClanInfoView::default()
    });
    m.data.clan.chat = vec![("Kin".into(), "LONGCHATLINE_OVER_THE_PAGE".into())];
    m.open(Go::Clan);
    Some(m)
}

fn click(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

#[test]
fn rom_web_window_is_drawn_after_the_chat_lines() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.input(MenuInput::Command(CMD_LEAVE));
    assert!(
        m.sheet()
            .part(WEB_WINDOW, WEB_WINDOW, WidgetKind::Window)
            .unwrap()
            .shown
    );
    let items = m.items_at(0.0);
    let pos = |want: &str| {
        items.iter().rposition(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => text.contains(want),
            _ => false,
        })
    };
    let chat = pos("LONGCHATLINE").expect("chat line drawn");
    // the window's title (the leave page's 0x500 text; a leader has no create page, CLI-1889) comes first
    let title = m.cat.text(0x500);
    let title_at = pos(&title).expect("title drawn");
    assert!(
        title_at > chat,
        "window 5 ({title_at}) drawn after the chat line ({chat})"
    );
}

#[test]
fn rom_web_window_takes_the_clicks_over_the_buttons_behind() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let notice = m
        .sheet()
        .parts
        .iter()
        .find(|p| p.kind == WidgetKind::Button && p.command == CMD_NOTICE)
        .map(|p| p.rect())
        .expect("notice button");
    let at = [
        (notice[0] + notice[2] / 2) as f32,
        (notice[1] + notice[3] / 2) as f32,
    ];
    m.input(MenuInput::Command(CMD_LEAVE));
    let own = m
        .sheet()
        .parts
        .iter()
        .any(|p| p.layer == WEB_WINDOW && p.kind != WidgetKind::Window && p.contains(at));
    assert!(!own, "the probe point is not on a window-5 control");
    click(&mut m, at);
    let shown = |m: &Menu| {
        m.sheet()
            .part(NOTICE_WINDOW, NOTICE_WINDOW, WidgetKind::Window)
            .unwrap()
            .shown
    };
    assert!(!shown(&m), "the button behind the open web window is dead");
    // closed again: the button works
    m.input(MenuInput::Command(CMD_WEB_CLOSE));
    click(&mut m, at);
    assert!(shown(&m), "the notice window opens once window 5 is closed");
}

/// Presses the visible button with `cmd` by its rectangle.
fn press_cmd(m: &mut Menu, cmd: i32) {
    let s = m.sheet();
    let i = (0..s.parts.len())
        .find(|&i| {
            s.parts[i].command == cmd && s.parts[i].kind == WidgetKind::Button && s.visible(i)
        })
        .unwrap_or_else(|| panic!("button {cmd} on the clan screen"));
    let p = &m.sheet().parts[i];
    let r = p.hit.unwrap_or([p.pos[0], p.pos[1], p.size[0], p.size[1]]);
    let c = [(r[0] + r[2] / 2) as f32, (r[1] + r[3] / 2) as f32];
    m.input(MenuInput::Move(c));
    click(m, c);
}

fn wait_screen(m: &mut Menu, name: &str) -> bool {
    for _ in 0..200 {
        m.update();
        if m.screen_name() == name {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// CLI-1260 (playtest report: "if I click Clan I can't click Shop or the other nav buttons without pressing Escape
/// first"): the lobby bar's Shop button leaves the clan screen, with and without the modal web window up.
#[test]
fn rom_the_bar_shop_button_works_on_the_clan_screen() {
    use dc_client::menu::shop::CMD_BAR_SHOP;
    for modal in [false, true] {
        let Some(mut m) = menu() else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        assert_eq!(m.screen_name(), "clan");
        if modal {
            m.input(MenuInput::Command(CMD_LEAVE));
        }
        press_cmd(&mut m, CMD_BAR_SHOP);
        assert!(wait_screen(&mut m, "shop"), "Shop from the clan screen (modal {modal}): {}", m.screen_name());
    }
}

/// CLI-1260: the bar's top-right X asks the quit question on the clan screen, also over the open notice window.
#[test]
fn rom_the_bar_x_asks_to_quit_on_the_clan_screen() {
    use dc_client::menu::shop::CMD_BAR_QUIT;
    for modal in [false, true] {
        let Some(mut m) = menu() else {
            eprintln!("skipped: no CA_ROM");
            return;
        };
        if modal {
            m.input(MenuInput::Command(CMD_NOTICE));
        }
        press_cmd(&mut m, CMD_BAR_QUIT);
        assert_eq!(m.screen_name(), "clan");
        assert!(m.popup_count() > 0, "the quit box is up (modal {modal})");
    }
}
