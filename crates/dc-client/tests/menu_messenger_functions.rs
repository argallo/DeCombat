//! lobby20 item 3: the messenger's functions as 2009 has them: add friend (the 0x4c9 input box, 16 characters),
//! remove (the friend row menu, then the 0x13b1 question), the friend request question (0x5ac) and the 1:1 chat
//! panel (whisper). Headless, no lobby: what the client asks of the relay is read from the link's latest request.
//!
//! ROM-gated (skipped without `CA_ROM`).

use dc_client::hud::items::DrawItem;
use dc_client::menu::frontfx::SOUND_MESSENGER_TWINKLE;
use dc_client::menu::link::{LinkEvent, LinkOptions};
use dc_client::menu::messenger::{
    new_request, request_box, Friend, CLAN_INVITE_TEXT, LIST_AT, ROW_HEIGHT, TREE_AT, WINDOW_AT,
};
use dc_client::menu::messenger::{BAR_FRIENDS_CMD, BUTTON_BLINK};
use dc_client::menu::popup::BoxTag;
use dc_client::menu::sheet::Look;
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::settings::QUICK_BLOCK_WHISPERS;
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
    // the unreachable lobby ends the link at once: take that box away
    for _ in 0..40 {
        m.update();
        if m.popup().is_some() {
            m.input(MenuInput::Key(MenuKey::Enter));
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    m.open(Go::Lobby);
    m.data.friends.me = "Me".into();
    m.data.friends.list = vec![
        Friend {
            name: "Ann".into(),
            online: true,
        },
        Friend {
            name: "Bo".into(),
            online: false,
        },
    ];
    m.toggle_messenger();
    Some(m)
}

fn click(m: &mut Menu, p: [f32; 2]) {
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
}

/// The centre of tree row `n` (0 = the group row, friends from 1).
fn row_at(n: i32) -> [f32; 2] {
    [
        (WINDOW_AT[0] + LIST_AT[0] + TREE_AT[0] + 60) as f32,
        (WINDOW_AT[1] + LIST_AT[1] + TREE_AT[1] + n * ROW_HEIGHT + 8) as f32,
    ]
}

fn texts(m: &mut Menu) -> Vec<String> {
    m.items_at(0.0)
        .iter()
        .filter_map(|i| match i {
            DrawItem::Text { text, .. } | DrawItem::Label { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn rom_add_friend_opens_the_name_box_and_sends_the_name() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    // the Add Friend caption (0x13f7) at the bottom of the window
    click(
        &mut m,
        [
            (WINDOW_AT[0] + 0x61 + 10) as f32,
            (WINDOW_AT[1] + 0x1ad + 5) as f32,
        ],
    );
    let p = m.popup().expect("the add friend box");
    assert_eq!(p.tag, BoxTag::AddFriend);
    assert_eq!(p.title, 0x1619);
    assert_eq!(p.text, m.cat.text(0x4c9));
    for c in "AbcdefghijKLMNOPQRS".chars() {
        m.input(MenuInput::Char(c));
    }
    // the typed text is shown (not masked) and stops at 16 characters
    assert_eq!(m.popup().unwrap().input, "AbcdefghijKLMNOP");
    assert!(texts(&mut m).iter().any(|t| t == "AbcdefghijKLMNOP|"));
    let sent = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_none());
    assert_eq!(m.link_sent(), sent + 1, "the add request");
    assert!(
        m.link_latest_request()
            .contains("!nm add\\tAbcdefghijKLMNOP"),
        "{}",
        m.link_latest_request()
    );
}

#[test]
fn rom_friend_menu_remove_asks_then_removes() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let p = row_at(2);
    m.input(MenuInput::PressAlt(p));
    let menu = m.messenger().menu.clone().expect("the friend row menu");
    assert_eq!(menu.name, "Bo");
    // the third entry is Remove (0x1412)
    let item = [(menu.at[0] + 20) as f32, (menu.at[1] + 2 * 20 + 10) as f32];
    assert!(texts(&mut m).iter().any(|t| *t == m.cat.text(0x1412)));
    click(&mut m, item);
    let q = m.popup().expect("the remove question");
    assert_eq!(q.tag, BoxTag::RemoveFriend);
    assert_eq!(q.text, m.cat.text(0x13b1).replace("%1!s!", "Bo"));
    // No leaves everything
    let sent = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.link_sent(), sent);
    // again, Yes (Enter)
    m.input(MenuInput::PressAlt(p));
    let menu = m.messenger().menu.clone().unwrap();
    click(&mut m, [(menu.at[0] + 20) as f32, (menu.at[1] + 50) as f32]);
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_sent(), sent + 1);
    assert!(
        m.link_latest_request().contains("!nm remove\\tBo"),
        "{}",
        m.link_latest_request()
    );
}

#[test]
fn rom_friend_request_asks_with_the_2009_text() {
    let Some(m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let e = LinkEvent::Messenger(vec!["request".into(), "Cy".into()]);
    assert_eq!(new_request(&m.data.friends, &e).as_deref(), Some("Cy"));
    // an open request does not ask again; other lines do not ask
    let mut open = m.data.friends.clone();
    open.requests.push("Cy".into());
    assert_eq!(new_request(&open, &e), None);
    assert_eq!(
        new_request(
            &m.data.friends,
            &LinkEvent::Messenger(vec!["status".into(), "x".into()])
        ),
        None
    );
    let q = request_box(&m.cat, "Cy");
    assert_eq!(q.tag, BoxTag::FriendRequest);
    assert_eq!(q.input, "Cy");
    assert_eq!(q.text, m.cat.text(0x5ac).replace("%s", "Cy"));
}

/// friend-accept (CLI-1898): the request box that pops up while the player types does not take the chat line's
/// Enter as Yes; Escape leaves the request open, No declines it and drops its row, only a click on Yes accepts.
#[test]
fn rom_friend_request_box_takes_no_enter() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let ask = |m: &mut Menu, name: &str| {
        m.feed_event(LinkEvent::Messenger(vec!["request".into(), name.into()]));
        m.update();
        let p = m.popup().expect("the request box");
        assert_eq!(p.tag, BoxTag::FriendRequest);
        assert_eq!(p.input, name);
    };
    ask(&mut m, "Cy");
    let sent = m.link_sent();
    m.input(MenuInput::Char('k'));
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_some(), "Enter does not answer the box");
    assert_eq!(m.link_sent(), sent, "nothing accepted");
    // Escape: closed, unanswered, the request stays a row (and a repeat of it does not ask again)
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(m.popup().is_none());
    assert_eq!(m.link_sent(), sent);
    assert_eq!(m.data.friends.requests, ["Cy"]);
    m.feed_event(LinkEvent::Messenger(vec!["request".into(), "Cy".into()]));
    m.update();
    assert!(m.popup().is_none());
    // No declines and the row goes
    ask(&mut m, "Di");
    let no = m.popup().unwrap().buttons()[1];
    click(&mut m, [(no[0] + 4) as f32, (no[1] + 4) as f32]);
    assert_eq!(m.link_sent(), sent + 1);
    assert!(
        m.link_latest_request().contains("!nm decline\\tDi"),
        "{}",
        m.link_latest_request()
    );
    assert_eq!(m.data.friends.requests, ["Cy"]);
    // a click on Yes accepts
    ask(&mut m, "Ed");
    let yes = m.popup().unwrap().buttons()[0];
    click(&mut m, [(yes[0] + 4) as f32, (yes[1] + 4) as f32]);
    assert!(m.popup().is_none());
    assert_eq!(m.link_sent(), sent + 2);
    assert!(
        m.link_latest_request().contains("!nm accept\\tEd"),
        "{}",
        m.link_latest_request()
    );
}

/// CLI-1901: a clan invitation (relay line `claninvite`, LG-420) asks with the clan's name on any screen, once per
/// clan; Enter does not answer it, Escape leaves it open, a click on No declines and on Yes joins.
#[test]
fn rom_clan_invite_box_answers_by_click_only() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let ask = |m: &mut Menu, name: &str, uid: u32| {
        let line = vec!["claninvite".into(), name.into(), uid.to_string()];
        m.feed_event(LinkEvent::Messenger(line));
        m.update();
        let p = m.popup().expect("the clan invitation box");
        assert_eq!(p.tag, BoxTag::ClanInvite(uid));
        assert_eq!(p.text, CLAN_INVITE_TEXT.replace("%s", name));
    };
    ask(&mut m, "Wolves", 4);
    // the same invitation again while the box is up: no second box
    m.feed_event(LinkEvent::Messenger(vec![
        "claninvite".into(),
        "Wolves".into(),
        "4".into(),
    ]));
    m.update();
    let sent = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Enter));
    assert!(m.popup().is_some(), "Enter does not answer the box");
    m.input(MenuInput::Key(MenuKey::Escape));
    assert!(m.popup().is_none(), "one box for the repeated line");
    assert_eq!(
        m.link_sent(),
        sent,
        "Escape sends nothing (the invitation stays open)"
    );
    ask(&mut m, "Ravens", 7);
    let no = m.popup().unwrap().buttons()[1];
    click(&mut m, [(no[0] + 4) as f32, (no[1] + 4) as f32]);
    assert_eq!(m.link_sent(), sent + 1);
    assert!(
        m.link_latest_request().contains("!nm clanno\\t7"),
        "{}",
        m.link_latest_request()
    );
    ask(&mut m, "Wolves", 4);
    let yes = m.popup().unwrap().buttons()[0];
    click(&mut m, [(yes[0] + 4) as f32, (yes[1] + 4) as f32]);
    assert!(m.popup().is_none());
    assert_eq!(m.link_sent(), sent + 2);
    assert!(
        m.link_latest_request().contains("!nm clanyes\\t4"),
        "{}",
        m.link_latest_request()
    );
}

#[test]
fn rom_chat_panel_shows_the_whispers_and_sends_text() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    // the relay delivers a whisper from Ann: it is kept in the conversation
    let line = ["whisper", "Ann", "Me", "hello there"].map(String::from);
    m.data.friends.apply(&line);
    // Chat in the row menu opens the panel
    let p = row_at(1);
    m.input(MenuInput::PressAlt(p));
    let menu = m.messenger().menu.clone().expect("menu");
    click(&mut m, [(menu.at[0] + 20) as f32, (menu.at[1] + 10) as f32]);
    assert_eq!(m.messenger().chat_with.as_deref(), Some("Ann"));
    let t = texts(&mut m);
    assert!(t.contains(&m.cat.text(0x5ab).replace("%s", "Ann")), "{t:?}");
    assert!(t.iter().any(|x| x == "Ann: hello there"), "{t:?}");
    // typing and Enter send the whisper
    for c in "hi back".chars() {
        m.input(MenuInput::Char(c));
    }
    let sent = m.link_sent();
    m.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(m.link_sent(), sent + 1);
    assert!(
        m.link_latest_request().contains("!nm w\\tAnn\\thi back"),
        "{}",
        m.link_latest_request()
    );
    // Escape leaves the panel, the window stays
    m.input(MenuInput::Key(MenuKey::Escape));
    assert_eq!(m.messenger().chat_with, None);
    assert!(m.messenger_open());
}

/// The bar's messenger button: its prefix and position on the current screen.
fn bar_button(m: &Menu) -> (String, [i32; 2]) {
    let s = m.sheet();
    let p = s
        .parts
        .iter()
        .find(|p| p.command == BAR_FRIENDS_CMD && p.kind == WidgetKind::Button)
        .expect("bar button");
    match &p.look {
        Look::Button { prefix, .. } => (prefix.clone(), p.pos),
        other => panic!("{other:?}"),
    }
}

/// The button texture drawn at `at` at clock `secs`.
fn button_tex(m: &mut Menu, prefix: &str, at: [i32; 2], secs: f32) -> Option<String> {
    m.items_at(secs).into_iter().find_map(|i| match i {
        DrawItem::Image { tex, dst, .. }
            if tex.starts_with(prefix) && dst[0] == at[0] as f32 && dst[1] == at[1] as f32 =>
        {
            Some(tex)
        }
        _ => None,
    })
}

fn feed(m: &mut Menu, line: &[&str]) {
    m.feed_event(LinkEvent::Messenger(
        line.iter().map(|s| s.to_string()).collect(),
    ));
    m.update();
}

#[test]
fn rom_a_waiting_whisper_blinks_the_bar_button_until_the_window_opens() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    // the helper opened the window: close it
    m.toggle_messenger();
    assert!(!m.messenger_open());
    assert_eq!(m.data.friends.unread, 0);
    let (prefix, at) = bar_button(&m);
    let plain = format!("{prefix}A.dtx");
    assert_eq!(
        button_tex(&mut m, &prefix, at, 0.1).as_deref(),
        Some(plain.as_str())
    );
    // the own (echoed) whisper does not light it
    feed(&mut m, &["whisper", "Me", "Ann", "yo"]);
    assert_eq!(m.data.friends.unread, 0);
    // Ann's whisper while closed: the button blinks (C texture in the even periods) and the twinkle plays
    let _ = m.take_sound_cues();
    feed(&mut m, &["whisper", "Ann", "Me", "hello"]);
    assert_eq!(m.data.friends.unread, 1);
    assert!(
        m.pending_sounds().contains(&SOUND_MESSENGER_TWINKLE),
        "{:?}",
        m.pending_sounds()
    );
    let lit = format!("{prefix}C.dtx");
    assert_eq!(
        button_tex(&mut m, &prefix, at, 0.1).as_deref(),
        Some(lit.as_str())
    );
    assert_eq!(
        button_tex(&mut m, &prefix, at, 0.1 + BUTTON_BLINK).as_deref(),
        Some(plain.as_str())
    );
    // opening the window clears it
    m.toggle_messenger();
    assert_eq!(m.data.friends.unread, 0);
    assert_eq!(
        button_tex(&mut m, &prefix, at, 0.1).as_deref(),
        Some(plain.as_str())
    );
    // a whisper while the window is open does not light it after closing
    feed(&mut m, &["whisper", "Ann", "Me", "again"]);
    m.toggle_messenger();
    assert_eq!(m.data.friends.unread, 0);
    // Block Whispers: another player's whisper never lights it
    m.data.settings.inert.insert(QUICK_BLOCK_WHISPERS.into(), 1);
    feed(&mut m, &["whisper", "Ann", "Me", "blocked"]);
    assert_eq!(m.data.friends.unread, 0);
    assert_eq!(
        button_tex(&mut m, &prefix, at, 0.1).as_deref(),
        Some(plain.as_str())
    );
    // a new friend request while closed lights it
    feed(&mut m, &["request", "Cy"]);
    assert_eq!(m.data.friends.unread, 1);
    assert_eq!(
        button_tex(&mut m, &prefix, at, 0.1).as_deref(),
        Some(lit.as_str())
    );
}
