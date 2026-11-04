//! w2-lobbychat (M6 / M25 / m62): the lobby and waiting-room chat through the menu's own inputs against a scripted
//! lobby peer on a loopback socket: typed characters and Enter (or a press on the chat page's Enter button) send the
//! 2009 chat frame 0x3a0; the line shows only when the server's echo 0x3051 arrives; a 0x3053 mute end refuses the
//! next line with the 2009 warning; a notice 0x367 opens its box; member sub-states (0x3042) show in the waiting
//! room; the draw list holds the chat edit box inside the narrowed chat page with nothing covered.
//!
//! ROM-gated (skipped without `CA_ROM`): the chat page comes from the user's layout pages. No lobby server runs: the
//! test's socket plays the lobby side, handed to the menu as a connection back from a match.
// SPEC: PROTOCOL_2009.md §3.6 (0x3a0 wstr[121] PROVEN cs+0x18a0d0; 0x3051), §6.4 (0x3053), §3.3 (0x367), §3.7 (0x3042)

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use dc_botclient::lobby::{AuthTicket, Lobby, LobbyProfile, RoomMember, RoomSeat, TcpLink};
use dc_botclient::record::Recorder;
use dc_client::hud::items::DrawItem;
use dc_client::menu::drawlist::{check, FontTable, Origin};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::lobby::CHAT_PANEL;
use dc_client::menu::room::SUB_AWAY;
use dc_client::menu::sheet::Look;
use dc_client::menu::textbox::{
    chat_box, CHAT_COLOUR, CHAT_EDIT_AT, LOBBY_CHAT_CLIP_RIGHT, ROOM_CHAT_CLIP_RIGHT,
    TEXT_MUTED_HOURS, TEXT_REPEATED, WARN_COLOUR,
};
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions};
use dc_ui_data::WidgetKind;
use dc_wire::{
    parse_tcp_frames, PktWriter, CHAT_PUSH, CHAT_SEND, MUTE_UNTIL_PUSH, NOTICE_REPLY,
    ROOM_USER_STATE_PUSH,
};

/// The lobby side of the menu's connection.
struct Peer {
    s: TcpStream,
    buf: Vec<u8>,
}

impl Peer {
    /// Frames that arrived within `wait` (id, payload with its inner header).
    fn frames(&mut self, wait: Duration) -> Vec<(u16, Vec<u8>)> {
        let end = Instant::now() + wait;
        let mut tmp = [0u8; 8192];
        self.s
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        while Instant::now() < end {
            if let Ok(n) = self.s.read(&mut tmp) {
                self.buf.extend_from_slice(&tmp[..n]);
            }
        }
        let (f, used) = parse_tcp_frames(&self.buf).unwrap();
        self.buf.drain(..used);
        f.into_iter().map(|(id, d)| (id as u16, d)).collect()
    }

    fn send(&mut self, w: &PktWriter) {
        self.s.write_all(&w.to_frame()).unwrap();
    }
}

fn wstr(d: &[u8], off: usize) -> String {
    let units: Vec<u16> = (off..d.len().saturating_sub(1))
        .step_by(2)
        .map(|i| u16::from_le_bytes([d[i], d[i + 1]]))
        .take_while(|u| *u != 0)
        .collect();
    String::from_utf16_lossy(&units)
}

/// A menu whose lobby connection is the test's socket, in the waiting room.
fn menu_with_peer(rom: std::path::PathBuf) -> (Menu, Peer) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mut menu = Menu::new(MenuOptions {
        auth: addr,
        user: "me".into(),
        link: LinkOptions::new("lobby-chat-test"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    let rec = Recorder::null("lobby-chat-test");
    let link = TcpLink::connect(addr, "lobby", &rec, Duration::from_secs(5)).unwrap();
    let (s, _) = listener.accept().unwrap();
    let profile = LobbyProfile {
        nick: "Me".into(),
        my_id: 7,
        ..LobbyProfile::default()
    };
    menu.data.profile = profile.clone();
    let ticket = AuthTicket {
        session_key: [0; 16],
        account_no: 1,
        lobby: addr,
        server_id: 1,
        server_name: String::new(),
        created_character: false,
    };
    let lobby = Lobby::from_parts(
        link,
        ticket,
        profile,
        Some(RoomSeat::default()),
        Duration::from_secs(5),
        rec,
    );
    menu.back_from_game(Some(lobby));
    assert_eq!(menu.screen_name(), "room");
    (menu, Peer { s, buf: Vec::new() })
}

fn type_text(menu: &mut Menu, text: &str) {
    for c in text.chars() {
        menu.input(MenuInput::Char(c));
    }
}

/// The text of every 0x3a0 among `frames`.
fn chat_sends(frames: &[(u16, Vec<u8>)]) -> Vec<String> {
    frames
        .iter()
        .filter(|(id, _)| *id == CHAT_SEND)
        .map(|(_, d)| {
            assert_eq!(d.len(), 0xf8, "0x3a0 is 0xf8 bytes");
            wstr(d, 6)
        })
        .collect()
}

fn echo(peer: &mut Peer, name: &str, text: &str) {
    let mut w = PktWriter::new(CHAT_PUSH);
    let len = text.encode_utf16().count() as u32;
    w.u32(7)
        .u32(name.encode_utf16().count() as u32)
        .u32(0)
        .wstr(name, 17)
        .u32(len)
        .wstr(text, len as usize + 1);
    peer.send(&w);
}

fn pump_until(menu: &mut Menu, what: impl Fn(&Menu) -> bool) -> bool {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        menu.update();
        if what(menu) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

fn label_at<'a>(items: &'a [DrawItem], text: &str) -> Option<(&'a [f32; 2], [u8; 4])> {
    items.iter().find_map(|i| match i {
        DrawItem::Label {
            text: t,
            pos,
            style,
            ..
        } if t == text => Some((pos, style.colour)),
        _ => None,
    })
}

#[test]
fn rom_typed_chat_goes_out_as_0x3a0_and_shows_with_the_echo() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, mut peer) = menu_with_peer(rom);
    let _ = peer.frames(Duration::from_millis(200)); // the waiting room's requests

    // waiting room: typed text and Enter -> 0x3a0; no line until the echo
    let edit = chat_box(menu.sheet()).expect("the waiting room builds the chat box");
    assert!(
        menu.sheet().parts[edit].focus,
        "the chat box has the keyboard"
    );
    type_text(&mut menu, "hello");
    menu.input(MenuInput::Key(MenuKey::Space));
    type_text(&mut menu, "room");
    menu.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(
        chat_sends(&peer.frames(Duration::from_millis(300))),
        ["hello room"]
    );
    let edit = chat_box(menu.sheet()).unwrap();
    assert!(
        menu.sheet().parts[edit].text.is_empty(),
        "the field is cleared"
    );
    assert!(
        menu.data.chat.is_empty(),
        "the client never prints its own line"
    );
    echo(&mut peer, "Me", "hello room");
    assert!(
        pump_until(&mut menu, |m| m
            .data
            .chat
            .iter()
            .any(|l| l == "Me : hello room")),
        "echo line: {:?}",
        menu.data.chat
    );
    let items = menu.items_at(0.0);
    let (_, colour) =
        label_at(&items, "Me : hello room").expect("the echo is drawn in the chat list");
    assert_eq!(colour, dc_client::menu::catalog::argb(CHAT_COLOUR));

    // m62: a member's away sub-state shows as its state string
    menu.data.members.insert(
        9,
        RoomMember {
            char_no: 9,
            name: "Zed".into(),
            team: 1,
            state: 0,
            exp: 0,
        },
    );
    let mut st = PktWriter::new(ROOM_USER_STATE_PUSH);
    st.u32(9).u32(0).u32(2);
    peer.send(&st);
    assert!(pump_until(&mut menu, |m| m.data.member_sub.get(&9) == Some(&2)));
    let away = menu.cat.text(SUB_AWAY);
    assert!(
        label_at(&menu.items_at(0.0), &away).is_some(),
        "away drawn on the member row"
    );

    // the room list: the page's Enter button sends too (a press on its rectangle)
    menu.open(Go::Lobby);
    let _ = peer.frames(Duration::from_millis(200));
    type_text(&mut menu, "hi lobby");
    let b = menu
        .sheet()
        .part(CHAT_PANEL, 0, WidgetKind::Button)
        .expect("chat Enter button")
        .rect();
    let c = [(b[0] + b[2] / 2) as f32, (b[1] + b[3] / 2) as f32];
    menu.input(MenuInput::Press(c));
    menu.input(MenuInput::Release(c));
    assert_eq!(
        chat_sends(&peer.frames(Duration::from_millis(300))),
        ["hi lobby"]
    );
    // an empty field sends nothing
    menu.input(MenuInput::Press(c));
    menu.input(MenuInput::Release(c));
    assert!(chat_sends(&peer.frames(Duration::from_millis(200))).is_empty());

    // 0x3053: muted for two hours and more -> the 2009 warning line, no 0x3a0
    let until = dc_botclient::clock::local_ticks() + 2 * 3600 * 10_000_000 + 600 * 10_000_000;
    let mut m = PktWriter::new(MUTE_UNTIL_PUSH);
    m.u32(until as u32).u32((until >> 32) as u32);
    peer.send(&m);
    assert!(pump_until(&mut menu, |m| m.data.muted_until == until));
    type_text(&mut menu, "muted?");
    menu.input(MenuInput::Key(MenuKey::Enter));
    assert!(chat_sends(&peer.frames(Duration::from_millis(300))).is_empty());
    let warn = dc_client::menu::roomtext::printf_int(&menu.cat.text(TEXT_MUTED_HOURS), 2);
    let k = menu
        .data
        .chat
        .iter()
        .position(|l| *l == warn)
        .expect("mute line");
    assert_eq!(menu.data.chat_colour_of(k), WARN_COLOUR);

    // 0x367: a notice opens its box
    let mut n = PktWriter::new(NOTICE_REPLY);
    n.u32(1)
        .u32(0)
        .u32(0)
        .wstr("Maintenance at noon", 513)
        .u32(0);
    peer.send(&n);
    assert!(pump_until(&mut menu, |m| m
        .popup()
        .is_some_and(|p| p.text == "Maintenance at noon")));
}

#[test]
fn rom_draw_list_has_the_chat_box_inside_the_narrowed_page_and_nothing_covered() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, _peer) = menu_with_peer(rom);
    for (go, right, panel) in [
        (Go::Lobby, LOBBY_CHAT_CLIP_RIGHT, 0),
        (Go::Room, ROOM_CHAT_CLIP_RIGHT, 1),
    ] {
        menu.open(go);
        type_text(
            &mut menu,
            "a line long enough to be cut at the end of the chat field: 0123456789",
        );
        let rows = menu.draw_trace();
        let sheet = menu.trace_sheet().clone();
        let fonts = FontTable::from_catalog(&menu.cat);
        let bad = check(&rows, &sheet, &fonts, &mut menu.cat);
        let covered: Vec<_> = bad.iter().filter(|m| m.class == "COVERED").collect();
        assert!(covered.is_empty(), "{go:?}: covered {covered:?}");
        // the edit box's text: at the 2009 place, inside its clip
        let edit = chat_box(&sheet).unwrap();
        let text = rows
            .iter()
            .find(|r| {
                matches!(r.origin, Origin::Part { part, .. } if part == edit)
                    && matches!(r.item, DrawItem::Label { .. })
            })
            .unwrap_or_else(|| panic!("{go:?}: the typed text is drawn"));
        assert_eq!(text.rect[0], CHAT_EDIT_AT[0] as f32, "{go:?}");
        assert_eq!(text.rect[1], CHAT_EDIT_AT[1] as f32, "{go:?}");
        assert!(
            text.rect[0] + text.rect[2] <= right as f32,
            "{go:?}: the text ends at {} past the field's {right}",
            text.rect[0] + text.rect[2]
        );
        // the chat window shows the screen's own panel (the room list's narrow one)
        let w = sheet
            .parts
            .iter()
            .find(|p| p.kind == WidgetKind::Window && p.layer == CHAT_PANEL)
            .unwrap();
        let want = menu.cat.paths.chat_panels[panel].clone();
        assert!(
            want.is_some(),
            "the panel literal resolves in the user's binary"
        );
        assert!(matches!(&w.look, Look::Panel { tex, .. } if Some(tex) == want.as_ref()));
        // the field is drawn after the chat window and before the character info / settings windows over it
        let win_seq = rows
            .iter()
            .find(|r| matches!(r.origin, Origin::Part { part, piece: 0 } if sheet.parts[part].kind == WidgetKind::Window && sheet.parts[part].layer == CHAT_PANEL))
            .map(|r| r.seq)
            .unwrap();
        assert!(text.seq > win_seq);
        // leave the field empty for the next screen's own box
        for _ in 0..80 {
            menu.input(MenuInput::Key(MenuKey::Backspace));
        }
    }
}

#[test]
fn rom_invitation_box_joins_with_the_password_and_the_notice_bar_scrolls() {
    use dc_client::menu::textbox::{TICKER_COLOUR, TICKER_LEFT, TICKER_RIGHT, TICKER_TOP};
    use dc_wire::{ROOM_ENTER, ROOM_INVITE_PUSH, TICKER_NOTICE_PUSH};
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, mut peer) = menu_with_peer(rom);
    menu.open(Go::Lobby);
    let _ = peer.frames(Duration::from_millis(200));
    // 0x3023 on the room list: a Yes / No box naming room number + 1 and the host; Yes enters with the password
    let mut i = PktWriter::new(ROOM_INVITE_PUSH);
    i.u32(4).wstr("HostX", 17).wstr("pw9", 10);
    peer.send(&i);
    assert!(pump_until(&mut menu, |m| m.popup().is_some()));
    let p = menu.popup().unwrap().clone();
    assert!(
        p.text.contains("HostX") && p.text.contains('5'),
        "{:?}",
        p.text
    );
    assert!(p.expires.is_some(), "the invitation closes by itself");
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    let (_, d) = f
        .iter()
        .find(|(id, _)| *id == ROOM_ENTER)
        .expect("0x384 after Yes");
    assert_eq!(u32::from_le_bytes([d[6], d[7], d[8], d[9]]), 4);
    assert_eq!(wstr(d, 0xa), "pw9");
    // 0x3052: the text enters the bar at its right end and moves left with the clock
    let mut t = PktWriter::new(TICKER_NOTICE_PUSH);
    t.u32(4).u32(1).u32(8).wstr("Big news", 9);
    peer.send(&t);
    assert!(pump_until(&mut menu, |m| m
        .data
        .ticker
        .active(Instant::now())));
    let find = |items: &[DrawItem]| {
        items.iter().find_map(|i| match i {
            DrawItem::Label {
                text, pos, style, ..
            } if "Big news".contains(text.as_str())
                && style.colour == dc_client::menu::catalog::argb(TICKER_COLOUR) =>
            {
                Some(pos[0])
            }
            _ => None,
        })
    };
    std::thread::sleep(Duration::from_millis(600));
    let a = find(&menu.items_at(0.0)).expect("the notice on the bar");
    std::thread::sleep(Duration::from_millis(600));
    let items = menu.items_at(0.0);
    let b = find(&items).expect("still on the bar");
    assert!(b < a, "moves left: {a} -> {b}");
    assert!((TICKER_LEFT..=TICKER_RIGHT).contains(&b));
    let y = items.iter().find_map(|i| match i {
        DrawItem::Label { text, pos, .. } if "Big news".contains(text.as_str()) => Some(pos[1]),
        _ => None,
    });
    assert_eq!(y, Some(TICKER_TOP));
    assert!(menu.needs_redraw(), "the bar keeps the screen drawing");
}

/// The same line a fourth time within a minute is refused with 0x1028 and, as in the 2009 send, the field is
/// cleared all the same.
// EVIDENCE: cshell+0x20ed0 (repeat branch: 0x1028 printed, then the edit control's clear cshell+0x2f4930)
#[test]
fn rom_a_repeated_line_is_refused_and_the_field_cleared() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, mut peer) = menu_with_peer(rom);
    let _ = peer.frames(Duration::from_millis(200));
    for _ in 0..3 {
        type_text(&mut menu, "again");
        menu.input(MenuInput::Key(MenuKey::Enter));
    }
    assert_eq!(
        chat_sends(&peer.frames(Duration::from_millis(300))),
        ["again", "again", "again"]
    );
    type_text(&mut menu, "again");
    menu.input(MenuInput::Key(MenuKey::Enter));
    assert!(chat_sends(&peer.frames(Duration::from_millis(200))).is_empty());
    let warning = menu.cat.text(TEXT_REPEATED);
    assert_eq!(menu.data.chat.last(), Some(&warning), "the 0x1028 line");
    let edit = chat_box(menu.sheet()).unwrap();
    assert!(
        menu.sheet().parts[edit].text.is_empty(),
        "the refused line leaves the field"
    );
}
