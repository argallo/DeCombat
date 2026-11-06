//! room-invite (CLI-1899): the waiting room's Invite button (command 30001, cshell+0x164810 -> cshell+0x15fd30).
//! Any member sends 0x396 (header only, cshell+0x18d960) while the room has a free place; a second press within 10 s
//! shows the wait box 0x50a (cshell+0x38f20); the lobby's 0x397 ok shows box 0x509 (0x50d on a clan server), a
//! refusal no box but allows the next invite at once (cshell+0x39060). A full room shows 0x50b, a clan room with both
//! sides taken 0x50e (cshell+0x15fd30). Runs without the ROM (string ids read back as `#<id>`).

use std::time::{Duration, Instant};

use dc_botclient::lobby::{LobbyProfile, RoomMember, ServerEntry};
use dc_client::menu::link::{LinkEvent, LinkOptions, RoomView};
use dc_client::menu::room::{
    invite_check, InviteRefusal, CMD_INVITE, INVITE_CLAN_SIDES_TAKEN, INVITE_ROOM_FULL,
    INVITE_SENT, INVITE_SENT_CLAN, INVITE_WAIT,
};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_client::testkit::synth_font;

fn menu() -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "InviteUser".into(),
        link: LinkOptions::new("room-invite"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m
}

/// A room of `members` (own player 1 on team 1, the others on team 2) with `max` places; the own player is no host.
fn room(m: &mut Menu, members: u32, max: i32, clan: bool) {
    let d = &mut m.data;
    d.profile = LobbyProfile {
        nick: "InviteUser".into(),
        my_id: 1,
        ..LobbyProfile::default()
    };
    if clan {
        d.server = Some(ServerEntry {
            id: 1,
            name: "clan".into(),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: 100,
            current: 1,
            clan_only: 1,
            op_capacity: 0,
            op_current: 0,
        });
    }
    d.room = RoomView {
        title: "invite".into(),
        host_id: 2,
        players: members as i32,
        max_players: max,
        goal: 50,
        ..RoomView::default()
    };
    for id in 1..=members {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: format!("P{id}"),
                team: if id == 1 { 1 } else { 2 },
                state: 0,
                exp: 0,
            },
        );
    }
    m.open(Go::Room);
}

fn popup_text(m: &Menu) -> Option<String> {
    m.popup().map(|p| p.text.clone())
}

fn close_box(m: &mut Menu) {
    m.input(MenuInput::Key(MenuKey::Enter));
    m.update();
    assert_eq!(m.popup_count(), 0);
}

#[test]
fn a_guest_invites_and_the_ok_reply_shows_box_0x509() {
    let mut m = menu();
    room(&mut m, 2, 8, false);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_INVITE));
    assert_eq!(m.take_link_sent(), vec!["InviteAll".to_string()]);
    assert_eq!(m.popup_count(), 0);
    m.feed_event(LinkEvent::InviteAllReply { ok: true });
    m.update();
    assert_eq!(popup_text(&m), Some(m.cat.text(INVITE_SENT)));
    close_box(&mut m);
    // a second press within 10 s: the wait box with the seconds left, nothing sent
    m.input(MenuInput::Command(CMD_INVITE));
    assert!(m.take_link_sent().is_empty());
    let wait = m.cat.text(INVITE_WAIT);
    assert_eq!(popup_text(&m), Some(wait));
}

#[test]
fn a_refused_invite_shows_no_box_and_allows_the_next_one() {
    let mut m = menu();
    room(&mut m, 2, 8, false);
    m.input(MenuInput::Command(CMD_INVITE));
    m.feed_event(LinkEvent::InviteAllReply { ok: false });
    m.update();
    assert_eq!(m.popup_count(), 0);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_INVITE));
    assert_eq!(m.take_link_sent(), vec!["InviteAll".to_string()]);
}

#[test]
fn a_full_room_shows_box_0x50b_and_sends_nothing() {
    let mut m = menu();
    room(&mut m, 4, 4, false);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_INVITE));
    assert!(m.take_link_sent().is_empty());
    assert_eq!(popup_text(&m), Some(m.cat.text(INVITE_ROOM_FULL)));
}

#[test]
fn a_clan_room_invites_while_a_side_is_free_and_answers_with_box_0x50d() {
    let mut m = menu();
    room(&mut m, 1, 16, true);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_INVITE));
    assert_eq!(m.take_link_sent(), vec!["InviteAll".to_string()]);
    m.feed_event(LinkEvent::InviteAllReply { ok: true });
    m.update();
    assert_eq!(popup_text(&m), Some(m.cat.text(INVITE_SENT_CLAN)));
    // both sides taken: box 0x50e
    let mut m = menu();
    room(&mut m, 2, 16, true);
    let _ = m.take_link_sent();
    m.input(MenuInput::Command(CMD_INVITE));
    assert!(m.take_link_sent().is_empty());
    assert_eq!(popup_text(&m), Some(m.cat.text(INVITE_CLAN_SIDES_TAKEN)));
}

#[test]
fn the_wait_counts_down_from_ten_seconds() {
    let t0 = Instant::now();
    let check = |at: Duration| invite_check(false, [1, 1], 0, 8, Some(t0), t0 + at);
    assert_eq!(check(Duration::ZERO), Err(InviteRefusal::Wait(10)));
    assert_eq!(
        check(Duration::from_millis(3500)),
        Err(InviteRefusal::Wait(7))
    );
    assert_eq!(
        check(Duration::from_millis(9999)),
        Err(InviteRefusal::Wait(1))
    );
    assert_eq!(check(Duration::from_secs(10)), Ok(()));
    assert_eq!(invite_check(false, [1, 1], 0, 8, None, t0), Ok(()));
    // the room checks come before the wait
    assert_eq!(
        invite_check(false, [4, 4], 0, 8, Some(t0), t0),
        Err(InviteRefusal::Box(INVITE_ROOM_FULL))
    );
}

/// The waiting page of the ROM layout has a visible Invite button whose command is 30001. ROM-gated.
#[test]
fn rom_the_waiting_room_shows_a_button_with_the_invite_command() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "InviteUser".into(),
        link: LinkOptions::new("room-invite"),
        rom: Some(rom),
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    room(&mut m, 2, 8, false);
    assert!(m.sheet().button(CMD_INVITE).is_some());
}
