//! gm-client (CLI-1300..CLI-1306): the operator privileges through the menu's own inputs against a scripted lobby
//! peer on a loopback socket (the waiting room, handed over as a connection back from a match): an operator's slash
//! lines go out as 0x3d0 / 0x3d2 / 0x3d4 and their replies print chat lines; a non-operator's slash line is chat;
//! `/k ` turns into the kick prompt and Enter kicks the named member (0x38e); the member menu lists Kick User for an
//! operator, asks, then kicks; a GM-named member is refused locally (0x1845) without a request.
//!
//! ROM-gated (skipped without `CA_ROM`): the pages, strings and the command words come from the user's ROM.
// SPEC: PROTOCOL_2009.md §6.4 (0x3d0..0x3d5), §3.7 (0x38e / 0x38f)

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use dc_botclient::lobby::{AuthTicket, Lobby, LobbyProfile, RoomMember, RoomSeat, TcpLink};
use dc_botclient::record::Recorder;
use dc_client::hud::items::DrawItem;
use dc_client::menu::gm::{
    TEXT_KICK, TEXT_KICKED, TEXT_KICK_ASK, TEXT_KICK_GM, TEXT_KICK_PROMPT,
};
use dc_client::menu::link::{LinkEvent, LinkOptions};
use dc_client::menu::popup::BoxTag;
use dc_client::menu::textbox::chat_box;
use dc_client::menu::{Menu, MenuInput, MenuKey, MenuOptions};
use dc_wire::{
    parse_tcp_frames, PktReader, PktWriter, CHAT_SEND, OP_LOOKUP, OP_LOOKUP_REPLY, OP_MUTE,
    OP_MUTE_REPLY, OP_RESTRICT, OP_RESTRICT_REPLY, ROOM_KICK, ROOM_KICK_REPLY,
};

struct Peer {
    s: TcpStream,
    buf: Vec<u8>,
}

impl Peer {
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

fn menu_with_peer(rom: std::path::PathBuf) -> (Menu, Peer) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let mut menu = Menu::new(MenuOptions {
        auth: addr,
        user: "me".into(),
        link: LinkOptions::new("gm-test"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    let rec = Recorder::null("gm-test");
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
    let mut peer = Peer { s, buf: Vec::new() };
    let _ = peer.frames(Duration::from_millis(200)); // the waiting room's requests
    (menu, peer)
}

fn type_text(menu: &mut Menu, text: &str) {
    for c in text.chars() {
        if c == ' ' {
            menu.input(MenuInput::Key(MenuKey::Space));
        } else {
            menu.input(MenuInput::Char(c));
        }
    }
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

fn set_operator(menu: &mut Menu, on: bool) {
    menu.feed_event(LinkEvent::Operator(on));
    menu.update();
    assert_eq!(menu.data.gm.operator, on);
}

fn ids(frames: &[(u16, Vec<u8>)]) -> Vec<u16> {
    frames.iter().map(|(id, _)| *id).collect()
}

fn frame(frames: &[(u16, Vec<u8>)], id: u16) -> Vec<u8> {
    frames
        .iter()
        .find(|(i, _)| *i == id)
        .unwrap_or_else(|| panic!("no {id:#x} in {:x?}", ids(frames)))
        .1
        .clone()
}

fn u32_at(d: &[u8], o: usize) -> u32 {
    PktReader::from_raw(d).at(o).u32()
}

fn wstr_at(d: &[u8], o: usize) -> String {
    PktReader::from_raw(d).at(o).wstr(17)
}

fn typed(menu: &Menu) -> String {
    let i = chat_box(menu.sheet()).unwrap();
    menu.sheet().parts[i].text.clone()
}

#[test]
fn rom_operator_slash_lines_send_the_2009_requests_and_print_the_replies() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, mut peer) = menu_with_peer(rom);
    for id in [
        0x480, 0x573, 0x13f7, 0x576, 0x1809, 0x180a, 0x180b, 0x180c, 0x1844, 0x1845, 0x1848, 0x1849,
    ] {
        println!("{id:#x}: {:?}", menu.cat.text(id));
    }
    let w = menu
        .cat
        .paths
        .gm
        .words
        .clone()
        .expect("the 2009 command words of the user's CShell.dll");

    // a player: the slash line is chat
    set_operator(&mut menu, false);
    type_text(&mut menu, &format!("/{} \"Bob\"", w.ban));
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    assert_eq!(ids(&f), [CHAT_SEND], "a player's slash line is chat");
    // LG-427: `/help` in the lobby chat goes to the lobby (which answers with its commands), for a player and an operator
    for (operator, line) in [(false, "/help"), (true, "/HELP now")] {
        set_operator(&mut menu, operator);
        type_text(&mut menu, line);
        menu.input(MenuInput::Key(MenuKey::Enter));
        let f = peer.frames(Duration::from_millis(300));
        assert_eq!(ids(&f), [CHAT_SEND], "{line} is sent (operator {operator})");
        assert_eq!(wstr_at(&frame(&f, CHAT_SEND), 6), line);
    }
    set_operator(&mut menu, false);

    // an operator: restrict 0x3d0, its reply 0x3d1 prints the confirmation with the name
    set_operator(&mut menu, true);
    type_text(&mut menu, &format!("/{} \"Bob\" -p 3 -r \"spam\"", w.ban));
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    assert_eq!(ids(&f), [OP_RESTRICT], "{f:x?}");
    let d = frame(&f, OP_RESTRICT);
    assert_eq!(d.len(), 0xb2);
    assert_eq!(wstr_at(&d, 6), "Bob");
    assert_eq!((u32_at(&d, 0x28), u32_at(&d, 0x2c)), (2, 3));
    assert_eq!(PktReader::from_raw(&d).at(0x30).wstr(65), "spam");
    assert!(typed(&menu).is_empty(), "the field is cleared");
    let mut r = PktWriter::new(OP_RESTRICT_REPLY);
    r.u32(1).u32(0).at(0xc);
    peer.send(&r);
    assert!(
        pump_until(&mut menu, |m| m.data.chat.iter().any(|l| l.contains("Bob"))),
        "restrict confirmation: {:?}",
        menu.data.chat
    );

    // lookup by account number, its reply prints two report lines
    let before = menu.data.chat.len();
    type_text(&mut menu, &format!("/{} -uid 42", w.trace));
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    let d = frame(&f, OP_LOOKUP);
    assert_eq!(d.len(), 0x2c);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (0, 42));
    let mut r = PktWriter::new(OP_LOOKUP_REPLY);
    r.u32(1).u32(0).u32(42).wstr("login", 17).wstr("Bob", 17).at(0x78);
    r.u32(1).u32(2).i32(-1).i32(0).i32(-1).u32(0).at(0x90);
    peer.send(&r);
    assert!(
        pump_until(&mut menu, |m| m.data.chat.len() >= before + 2),
        "lookup report: {:?}",
        menu.data.chat
    );
    assert!(menu.data.chat[before].contains("Bob") && menu.data.chat[before].contains("42"));

    // mute 0x3d4, its reply 0x3d5 prints the 2009 confirmation with the name and the number
    type_text(&mut menu, &format!("/{} \"Bob\" -t 2", w.punish));
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    let d = frame(&f, OP_MUTE);
    assert_eq!(d.len(), 0xb2);
    assert_eq!(wstr_at(&d, 6), "Bob");
    assert_eq!((u32_at(&d, 0x28), u32_at(&d, 0x2c)), (1, 2));
    let mut r = PktWriter::new(OP_MUTE_REPLY);
    r.u32(1).u32(0).at(0xc);
    peer.send(&r);
    assert!(
        pump_until(&mut menu, |m| m
            .data
            .chat
            .last()
            .is_some_and(|l| l.contains("Bob") && l.contains('2'))),
        "mute confirmation: {:?}",
        menu.data.chat
    );

    // CLI-1301 amended: an unknown word goes out as chat with its text (the lobby's /notice etc.)
    type_text(&mut menu, "/notice hello");
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    assert_eq!(ids(&f), [CHAT_SEND], "{f:x?}");
    let d = frame(&f, CHAT_SEND);
    let units: Vec<u16> = d[6..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    assert!(
        String::from_utf16_lossy(&units).contains("/notice hello"),
        "the chat packet carries the text"
    );
}

fn seat_members(menu: &mut Menu, host: u32) {
    let m = |char_no: i32, name: &str, team: i32| RoomMember {
        char_no,
        name: name.into(),
        team,
        state: 0,
        exp: 0,
    };
    menu.data.members.insert(7, m(70, "Me", 1));
    menu.data.members.insert(9, m(99, "Bob", 2));
    menu.data.members.insert(11, m(111, "GMfoo", 2));
    menu.data.room.host_id = host;
    menu.data.room.max_players = 16;
}

fn label_pos(items: &[DrawItem], text: &str) -> Option<[f32; 2]> {
    items.iter().find_map(|i| match i {
        DrawItem::Label { text: t, pos, .. } if t == text => Some(*pos),
        _ => None,
    })
}

fn click(menu: &mut Menu, p: [f32; 2]) {
    menu.input(MenuInput::Press(p));
    menu.input(MenuInput::Release(p));
}

/// Opens the member menu on `name`'s row and gives the drawn entry labels.
fn open_member_menu(menu: &mut Menu, name: &str) -> Vec<DrawItem> {
    let items = menu.items_at(0.0);
    let at = label_pos(&items, name).unwrap_or_else(|| panic!("{name}'s row is drawn"));
    menu.input(MenuInput::PressAlt([at[0] + 2.0, at[1] + 2.0]));
    menu.items_at(0.0)
}

#[test]
fn rom_kick_prompt_kicks_the_typed_member() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, mut peer) = menu_with_peer(rom);
    seat_members(&mut menu, 9);
    set_operator(&mut menu, true);
    let prompt = menu.cat.text(TEXT_KICK_PROMPT);
    assert!(!prompt.is_empty());
    type_text(&mut menu, "/k ");
    assert_eq!(typed(&menu), prompt, "`/k ` becomes the kick prompt");
    type_text(&mut menu, "Bob");
    menu.input(MenuInput::Key(MenuKey::Enter));
    let f = peer.frames(Duration::from_millis(300));
    assert_eq!(ids(&f), [ROOM_KICK]);
    let d = frame(&f, ROOM_KICK);
    assert_eq!(d.len(), 0x12);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (9, 99));
    assert!(typed(&menu).is_empty());
    let mut r = PktWriter::new(ROOM_KICK_REPLY);
    r.u32(1).u32(0).at(0xe);
    peer.send(&r);
    let kicked = menu.cat.text(TEXT_KICKED).replacen("%s", "Bob", 1);
    assert!(
        pump_until(&mut menu, |m| m.popup().is_some_and(|p| p.text == kicked)),
        "0x1849 box: {:?}",
        menu.popup().map(|p| p.text.clone())
    );

    // a name not in the room: the red line 0x57c, nothing sent
    while menu.popup().is_some() {
        let b = menu.popup().unwrap().buttons()[0];
        click(&mut menu, [(b[0] + b[2] / 2) as f32, (b[1] + b[3] / 2) as f32]);
    }
    type_text(&mut menu, "/k Nobody");
    menu.input(MenuInput::Key(MenuKey::Enter));
    assert!(peer.frames(Duration::from_millis(200)).is_empty());
    assert_eq!(
        menu.data.chat.last(),
        Some(&menu.cat.text(dc_client::menu::gm::TEXT_NO_MEMBER))
    );

    // Backspace into the prompt clears it; the next line is chat again
    let mut menu2 = menu;
    while menu2.popup().is_some() {
        let b = menu2.popup().unwrap().buttons()[0];
        click(&mut menu2, [(b[0] + b[2] / 2) as f32, (b[1] + b[3] / 2) as f32]);
    }
    type_text(&mut menu2, "/k ");
    menu2.input(MenuInput::Key(MenuKey::Backspace));
    menu2.input(MenuInput::Key(MenuKey::Backspace));
    assert!(typed(&menu2).is_empty(), "{:?}", typed(&menu2));
    type_text(&mut menu2, "x");
    menu2.input(MenuInput::Key(MenuKey::Enter));
    assert_eq!(ids(&peer.frames(Duration::from_millis(300))), [CHAT_SEND]);

    // a player's `/k ` stays text
    set_operator(&mut menu2, false);
    type_text(&mut menu2, "/k ");
    assert_eq!(typed(&menu2), "/k ");
}

#[test]
fn rom_member_menu_kick_entry_asks_then_kicks_and_spares_gm_names() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    let (mut menu, mut peer) = menu_with_peer(rom);
    let kick = menu.cat.text(TEXT_KICK);
    assert!(!kick.is_empty());

    // a guest without the operator flag: no Kick User
    seat_members(&mut menu, 9);
    set_operator(&mut menu, false);
    let items = open_member_menu(&mut menu, "Bob");
    assert!(label_pos(&items, &menu.cat.text(0x573)).is_some(), "the menu opens");
    assert!(label_pos(&items, &kick).is_none());
    menu.input(MenuInput::Press([5.0, 5.0]));

    // the host with the room badge (badge level set): Kick User
    seat_members(&mut menu, 7);
    menu.data.char_extras.room_badge = true;
    menu.data.room.badge = 1;
    let items = open_member_menu(&mut menu, "Bob");
    assert!(label_pos(&items, &kick).is_some(), "badge host");
    menu.input(MenuInput::Press([5.0, 5.0]));
    // the host without a badge: none (the plain host has no kick in 2009)
    menu.data.room.badge = 0;
    let items = open_member_menu(&mut menu, "Bob");
    assert!(label_pos(&items, &kick).is_none(), "plain host");
    menu.input(MenuInput::Press([5.0, 5.0]));

    // an operator guest: Kick User, the question, Yes -> 0x38e
    seat_members(&mut menu, 9);
    set_operator(&mut menu, true);
    let items = open_member_menu(&mut menu, "Bob");
    let at = label_pos(&items, &kick).expect("operator: Kick User");
    menu.input(MenuInput::Press([at[0] + 2.0, at[1] + 2.0]));
    let ask = menu.cat.text(TEXT_KICK_ASK).replacen("%s", "Bob", 1);
    let p = menu.popup().expect("the kick question");
    assert_eq!(p.text, ask);
    assert_eq!(p.tag, BoxTag::Kick { id: 9, char_no: 99 });
    let b = p.buttons()[0];
    click(&mut menu, [(b[0] + b[2] / 2) as f32, (b[1] + b[3] / 2) as f32]);
    let f = peer.frames(Duration::from_millis(300));
    let d = frame(&f, ROOM_KICK);
    assert_eq!((u32_at(&d, 6), u32_at(&d, 0xa)), (9, 99));

    // a GM-named member: 0x1845 here, nothing sent
    let items = open_member_menu(&mut menu, "GMfoo");
    let at = label_pos(&items, &kick).expect("operator: Kick User");
    menu.input(MenuInput::Press([at[0] + 2.0, at[1] + 2.0]));
    assert_eq!(
        menu.popup().map(|p| p.text.clone()),
        Some(menu.cat.text(TEXT_KICK_GM))
    );
    assert!(peer.frames(Duration::from_millis(200)).is_empty());

    // the lobby's refusal 0x38f reason 0x82 shows 0x1845 too
    while menu.popup().is_some() {
        let b = menu.popup().unwrap().buttons()[0];
        click(&mut menu, [(b[0] + b[2] / 2) as f32, (b[1] + b[3] / 2) as f32]);
    }
    let mut r = PktWriter::new(ROOM_KICK_REPLY);
    r.u32(0).u32(0x82).at(0xe);
    peer.send(&r);
    let gm = menu.cat.text(TEXT_KICK_GM);
    assert!(pump_until(&mut menu, |m| m
        .popup()
        .is_some_and(|p| p.text == gm)));
}
