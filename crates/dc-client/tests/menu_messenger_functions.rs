//! lobby20 item 3: the messenger's functions as 2009 has them: add friend (the 0x4c9 input box, 16 characters),
//! remove (the friend row menu, then the 0x13b1 question), the friend request question (0x5ac) and the 1:1 chat
//! panel (whisper). Headless, no lobby: what the client asks of the relay is read from the link's latest request.
//!
//! ROM-gated (skipped without `CA_ROM`).

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{LinkEvent, LinkOptions};
use dc_client::menu::messenger::{
    new_request, request_box, Friend, LIST_AT, ROW_HEIGHT, TREE_AT, WINDOW_AT,
};
use dc_client::menu::popup::BoxTag;
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};

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
