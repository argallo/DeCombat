//! CLI-1810: the report box's bug and comment categories, in the lobby box and the in-match form (our own layout,
//! no ROM). The typed description is drawn (visible text colour, a caret with a height), a space typed as the
//! menu's Space key lands in the entry, the name entry is hidden and the find button off on bug and comments and back
//! on player abuse, the name row is shaded while off, and a bug report goes out without a target name. The
//! description keeps its 600-unit cap.

use dc_client::hud::DrawItem;
use dc_client::menu::link::{report_packet, LinkCmd, LinkOptions};
use dc_client::menu::report::{
    self, CMD_FIND, CMD_GENERAL, CMD_SEND, FIND_ID, MESSAGE_ID, NAME_ROW_SHADE, TARGET_ID,
    TEXT_UNITS, WINDOW,
};
use dc_client::menu::reportdlg::{entry_text, FormStep, ReportForm};
use dc_client::menu::sheet::Sheet;
use dc_client::menu::{Go, Menu, MenuInput, MenuKey, MenuOptions, CMD_BAR_ABUSE};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

const SHADE: [u8; 4] = [
    (NAME_ROW_SHADE >> 16) as u8,
    (NAME_ROW_SHADE >> 8) as u8,
    NAME_ROW_SHADE as u8,
    (NAME_ROW_SHADE >> 24) as u8,
];

fn form() -> ReportForm {
    let players = ["Bot1".to_string(), "Me".to_string()];
    ReportForm::open(
        None,
        Some((synth_font(), 0)),
        (1024, 768),
        &players,
        "Me",
        None,
    )
}

fn lobby() -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("report-bug"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.open(Go::Lobby);
    m.input(MenuInput::Command(CMD_BAR_ABUSE));
    assert_eq!(m.screen_name(), report::NAME);
    m
}

fn centre(s: &Sheet, id: i32) -> [f32; 2] {
    let p = s.part(WINDOW, id, WidgetKind::EditBox).expect("edit box");
    let h = p.hit.unwrap_or([p.pos[0], p.pos[1], p.size[0], p.size[1]]);
    [
        h[0] as f32 + h[2] as f32 / 2.0,
        h[1] as f32 + h[3] as f32 / 2.0,
    ]
}

fn centre_of_command(s: &Sheet, cmd: i32) -> [f32; 2] {
    let p = s
        .parts
        .iter()
        .find(|p| p.command == cmd && p.kind == WidgetKind::Button)
        .expect("button");
    [
        p.pos[0] as f32 + p.size[0] as f32 / 2.0,
        p.pos[1] as f32 + p.size[1] as f32 / 2.0,
    ]
}

/// (target entry shown, find button enabled)
fn name_state(s: &Sheet) -> (bool, bool) {
    (
        s.part(WINDOW, TARGET_ID, WidgetKind::EditBox)
            .is_some_and(|p| p.shown),
        s.part(WINDOW, FIND_ID, WidgetKind::Button)
            .is_some_and(|p| p.enabled),
    )
}

fn shaded(items: &[DrawItem]) -> bool {
    items
        .iter()
        .any(|i| matches!(i, DrawItem::Fill { colour, .. } if *colour == SHADE))
}

/// The typed text is drawn with a visible colour, and the focused entry's caret has a height.
fn drawn_visibly(items: &[DrawItem], text: &str) {
    let label = items
        .iter()
        .find_map(|i| match i {
            DrawItem::Label { text: t, style, .. } if t == text => Some(style.colour),
            _ => None,
        })
        .unwrap_or_else(|| panic!("{text:?} drawn"));
    assert!(label[3] > 0, "the text is not transparent: {label:?}");
    assert!(
        items.iter().any(
            |i| matches!(i, DrawItem::Fill { rect, colour } if rect[2] == 2.0 && rect[3] > 0.0 && colour[3] > 0)
        ),
        "a caret with a height"
    );
}

// --- the in-match form ---

fn form_click(f: &mut ReportForm, p: [f32; 2]) -> FormStep {
    f.input(MenuInput::Move(p));
    f.input(MenuInput::Press(p));
    f.input(MenuInput::Release(p))
}

fn form_pick_kind(f: &mut ReportForm, kind: usize) {
    let at = centre_of_command(f.sheet().unwrap(), CMD_GENERAL);
    form_click(f, at);
    let p = f.dropdown().expect("general list").row_point(kind).unwrap();
    form_click(f, p);
    assert!(f.dropdown().is_none());
}

#[test]
fn form_bug_report_types_and_sends_without_a_target() {
    let mut f = form();
    // a name typed under player abuse first: it must not ride along on the bug report
    let at = centre(f.sheet().unwrap(), TARGET_ID);
    form_click(&mut f, at);
    for c in "Bot1".chars() {
        f.input(MenuInput::Char(c));
    }
    assert_eq!(name_state(f.sheet().unwrap()), (true, true));
    assert!(!shaded(&f.items()));
    form_pick_kind(&mut f, 1);
    assert_eq!(
        f.report().unwrap().category(),
        (1, 0),
        "Report a Bug, first detail"
    );
    assert_eq!(
        name_state(f.sheet().unwrap()),
        (false, false),
        "no name on a bug"
    );
    assert!(shaded(&f.items()), "the name row is shaded");
    // the description has the keyboard already; type into it without a click
    for c in "map hole".chars() {
        f.input(MenuInput::Char(c));
    }
    assert_eq!(entry_text(&f, MESSAGE_ID), "map hole");
    drawn_visibly(&f.items(), "map hole");
    // the find button is off: its click opens no list
    let at = centre_of_command(f.sheet().unwrap(), CMD_FIND);
    form_click(&mut f, at);
    assert!(f.dropdown().is_none());
    let at = centre_of_command(f.sheet().unwrap(), CMD_SEND);
    assert_eq!(form_click(&mut f, at), FormStep::Redraw, "sent notice");
    let sent = f.take_sent();
    let [LinkCmd::Report {
        name,
        text,
        kind,
        detail,
    }] = sent.as_slice()
    else {
        panic!("{sent:?}");
    };
    assert_eq!(
        (name.as_str(), text.as_str(), *kind, *detail),
        ("", "map hole", 1, 0)
    );
    let frame = report_packet(name, text, *kind, *detail).into_payload();
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x312);
    assert_eq!(u32::from_le_bytes(frame[10..14].try_into().unwrap()), 1);
    assert!(frame[0x12..0x34].iter().all(|&b| b == 0), "no target name");
    let typed: Vec<u8> = "map hole"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    assert_eq!(&frame[0x34..0x34 + typed.len()], typed.as_slice());
}

#[test]
fn form_name_row_follows_the_kind_both_ways() {
    let mut f = form();
    for (kind, want) in [(2, false), (0, true), (1, false), (0, true)] {
        form_pick_kind(&mut f, kind);
        assert_eq!(name_state(f.sheet().unwrap()), (want, want), "kind {kind}");
        assert_eq!(shaded(&f.items()), !want, "kind {kind}");
    }
    // back on player abuse the name entry takes the keyboard and draws what is typed
    for c in "Bot1".chars() {
        f.input(MenuInput::Char(c));
    }
    assert_eq!(entry_text(&f, TARGET_ID), "Bot1");
    drawn_visibly(&f.items(), "Bot1");
}

#[test]
fn form_description_keeps_its_600_unit_cap() {
    let mut f = form();
    form_pick_kind(&mut f, 2);
    for _ in 0..TEXT_UNITS + 50 {
        f.input(MenuInput::Char('x'));
    }
    assert_eq!(entry_text(&f, MESSAGE_ID).chars().count(), TEXT_UNITS);
}

// --- the lobby box ---

fn lobby_pick_kind(m: &mut Menu, kind: usize) {
    m.input(MenuInput::Command(CMD_GENERAL));
    let p = m.dropdown().expect("general list").row_point(kind).unwrap();
    m.input(MenuInput::Press(p));
    m.input(MenuInput::Release(p));
    assert!(m.dropdown().is_none());
}

fn lobby_message(m: &Menu) -> String {
    m.sheet()
        .part(WINDOW, MESSAGE_ID, WidgetKind::EditBox)
        .map(|p| p.text.clone())
        .unwrap_or_default()
}

#[test]
fn lobby_bug_report_types_and_sends_without_a_target() {
    let mut m = lobby();
    assert_eq!(name_state(m.sheet()), (true, true), "opens on player abuse");
    assert!(!shaded(&m.items()));
    lobby_pick_kind(&mut m, 1);
    assert_eq!(name_state(m.sheet()), (false, false), "no name on a bug");
    assert!(shaded(&m.items()));
    // a click on the description, then the typed text: the window delivers Space as a key
    let at = centre(m.sheet(), MESSAGE_ID);
    m.input(MenuInput::Move(at));
    m.input(MenuInput::Press(at));
    m.input(MenuInput::Release(at));
    for c in "map".chars() {
        m.input(MenuInput::Char(c));
    }
    m.input(MenuInput::Key(MenuKey::Space));
    for c in "hole".chars() {
        m.input(MenuInput::Char(c));
    }
    assert_eq!(lobby_message(&m), "map hole");
    drawn_visibly(&m.items(), "map hole");
    // the find button's command opens no list
    m.input(MenuInput::Command(CMD_FIND));
    assert!(m.dropdown().is_none());
    m.take_link_sent();
    m.input(MenuInput::Command(CMD_SEND));
    let sent = m.take_link_sent();
    assert_eq!(sent, vec!["Report(, 1/0, 8 chars)".to_string()], "{sent:?}");
}

#[test]
fn lobby_name_row_follows_the_kind_and_the_cap_holds() {
    let mut m = lobby();
    for (kind, want) in [(2, false), (0, true), (1, false)] {
        lobby_pick_kind(&mut m, kind);
        assert_eq!(name_state(m.sheet()), (want, want), "kind {kind}");
        assert_eq!(shaded(&m.items()), !want, "kind {kind}");
    }
    m.input(MenuInput::Paste("y".repeat(TEXT_UNITS + 10)));
    assert_eq!(lobby_message(&m).chars().count(), TEXT_UNITS);
    // player abuse still refuses an empty target
    lobby_pick_kind(&mut m, 0);
    m.take_link_sent();
    m.input(MenuInput::Command(CMD_SEND));
    assert!(m.take_link_sent().is_empty());
    assert!(m.popup().is_some(), "the target refusal");
}
