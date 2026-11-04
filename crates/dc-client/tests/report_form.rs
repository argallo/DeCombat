//! Round 10 (CLI-950): the in-match report form behind the exit box's middle button. Our own layout (no ROM): the
//! draw list at two window sizes through the HUD overlay, the entries, the refusal, the send command and its 0x312
//! frame, the ten-second notice. The ROM variant needs `CA_ROM` (nothing of the ROM in this file).

use dc_client::hud::compose::{resolve, Prim};
use dc_client::hud::{Hud, HudState};
use dc_client::menu::link::{report_packet, LinkCmd};
use dc_client::menu::report::{
    CMD_CANCEL, CMD_DETAIL, CMD_GENERAL, CMD_SEND, MESSAGE_ID, TARGET_ID, WINDOW,
};
use dc_client::menu::reportdlg::{entry_text, FormStep, ReportForm};
use dc_client::menu::{MenuInput, MenuKey};
use dc_client::testkit::synth_font;
use dc_ui_data::WidgetKind;

const SIZES: [(u32, u32); 2] = [(1024, 768), (1920, 1080)];

fn form(last: Option<std::time::Instant>) -> ReportForm {
    let players = ["Bot1".to_string(), "Bot2".to_string(), "Me".to_string()];
    ReportForm::open(
        None,
        Some((synth_font(), 0)),
        (1024, 768),
        &players,
        "Me",
        last,
    )
}

fn centre_of_command(f: &ReportForm, cmd: i32) -> [f32; 2] {
    let s = f.sheet().expect("the box");
    let p = s
        .parts
        .iter()
        .find(|p| p.command == cmd && p.kind == WidgetKind::Button)
        .unwrap_or_else(|| panic!("button {cmd:#x}"));
    [
        p.pos[0] as f32 + p.size[0] as f32 / 2.0,
        p.pos[1] as f32 + p.size[1] as f32 / 2.0,
    ]
}

fn centre_of_edit(f: &ReportForm, id: i32) -> [f32; 2] {
    let s = f.sheet().expect("the box");
    let p = s.part(WINDOW, id, WidgetKind::EditBox).expect("edit box");
    let h = p.hit.unwrap_or([p.pos[0], p.pos[1], p.size[0], p.size[1]]);
    [
        h[0] as f32 + h[2] as f32 / 2.0,
        h[1] as f32 + h[3] as f32 / 2.0,
    ]
}

fn click(f: &mut ReportForm, p: [f32; 2]) -> FormStep {
    f.input(MenuInput::Move(p));
    f.input(MenuInput::Press(p));
    f.input(MenuInput::Release(p))
}

fn type_text(f: &mut ReportForm, t: &str) {
    for c in t.chars() {
        f.input(MenuInput::Char(c));
    }
}

#[test]
fn the_form_draws_over_the_hud_at_two_window_sizes() {
    let mut f = form(None);
    let items = f.items();
    assert!(!items.is_empty(), "the form draws something");
    for (w, h) in SIZES {
        let mut hud = Hud::fallback(w, h, Some((synth_font(), 0)));
        let st = HudState::default();
        let plain = hud.items(&st).len();
        hud.overlay = items.clone();
        let with = hud.items(&st);
        assert_eq!(
            with.len(),
            plain + items.len(),
            "overlay appended at {w}x{h}"
        );
        // every virtual item resolves at the real size; text lands inside the target
        let prims = resolve(&with, &mut hud.assets, w, h);
        let drawn = prims.len();
        assert!(drawn > 0, "something drawn at {w}x{h}");
        for p in &prims {
            if let Prim::Fill { rect, .. } = p {
                assert!(
                    rect[0] >= 0 && rect[1] >= 0,
                    "fill inside the target: {rect:?}"
                );
            }
        }
    }
}

#[test]
fn a_report_goes_out_as_the_0x312_frame_with_its_kind_and_detail() {
    let mut f = form(None);
    // nothing typed: the refusal box, nothing sent
    let send = centre_of_command(&f, CMD_SEND);
    click(&mut f, send);
    assert!(f.popup().is_some(), "refusal for the missing player name");
    assert!(f.take_sent().is_empty());
    assert_eq!(f.input(MenuInput::Key(MenuKey::Enter)), FormStep::Redraw);
    assert!(f.popup().is_none());
    // the target, then the description
    let at = centre_of_edit(&f, TARGET_ID);
    click(&mut f, at);
    type_text(&mut f, "Bot1");
    let at = centre_of_edit(&f, MESSAGE_ID);
    click(&mut f, at);
    type_text(&mut f, "wall hack");
    assert_eq!(entry_text(&f, TARGET_ID), "Bot1");
    assert_eq!(entry_text(&f, MESSAGE_ID), "wall hack");
    let send = centre_of_command(&f, CMD_SEND);
    let step = click(&mut f, send);
    assert_eq!(step, FormStep::Redraw, "a sent report says so first");
    assert!(f.popup().is_some() && f.sheet().is_none());
    assert_eq!(
        f.input(MenuInput::Key(MenuKey::Enter)),
        FormStep::Closed,
        "the notice's OK closes the form"
    );
    let sent = f.take_sent();
    assert_eq!(sent.len(), 1, "{sent:?}");
    let LinkCmd::Report {
        name,
        text,
        kind,
        detail,
    } = sent[0].clone()
    else {
        panic!("not a report: {:?}", sent[0]);
    };
    assert_eq!(
        (name.as_str(), text.as_str(), kind, detail),
        ("Bot1", "wall hack", 0, 0)
    );
    assert!(f.sent_at().is_some());
    // the frame the window hands to the lobby connection
    let frame = report_packet(&name, &text, kind, detail).into_payload();
    assert_eq!(frame.len(), 0xe4c);
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x312);
    assert_eq!(
        u32::from_le_bytes(frame[10..14].try_into().unwrap()),
        0,
        "kind"
    );
    assert_eq!(
        u32::from_le_bytes(frame[14..18].try_into().unwrap()),
        0,
        "detail"
    );
}

#[test]
fn the_lobby_lists_pick_a_kind_and_the_detail_list_follows() {
    let mut f = form(None);
    // general list: three kinds; pick the second (no player needed)
    let at = centre_of_command(&f, CMD_GENERAL);
    click(&mut f, at);
    let d = f.dropdown().expect("general list open");
    assert_eq!(d.values.len(), 3);
    let row = (0..768)
        .flat_map(|y| (0..1024).map(move |x| [x as f32, y as f32]))
        .find(|p| d.row_at(*p) == Some(1))
        .expect("row 1 on screen");
    click(&mut f, row);
    assert!(f.dropdown().is_none(), "a pick closes the list");
    let at = centre_of_edit(&f, MESSAGE_ID);
    click(&mut f, at);
    type_text(&mut f, "map hole");
    let at = centre_of_command(&f, CMD_DETAIL);
    click(&mut f, at);
    assert_eq!(
        f.dropdown().expect("detail list").values.len(),
        6,
        "kind 1 has six details"
    );
    let at = centre_of_command(&f, CMD_DETAIL);
    click(&mut f, at);
    let send = centre_of_command(&f, CMD_SEND);
    let step = click(&mut f, send);
    assert_eq!(step, FormStep::Redraw, "the sent notice first");
    match f.take_sent().remove(0) {
        LinkCmd::Report { kind, detail, .. } => assert_eq!((kind, detail), (1, 0)),
        c => panic!("{c:?}"),
    }
}

#[test]
fn a_second_report_within_ten_seconds_shows_the_notice_only() {
    let mut f = form(Some(std::time::Instant::now()));
    assert!(f.sheet().is_none(), "no box, only the notice");
    assert!(f.popup().is_some());
    assert_eq!(f.input(MenuInput::Key(MenuKey::Enter)), FormStep::Closed);
    assert!(f.take_sent().is_empty());
}

#[test]
fn cancel_and_escape_close_without_sending() {
    let mut f = form(None);
    let at = centre_of_command(&f, CMD_CANCEL);
    assert_eq!(click(&mut f, at), FormStep::Closed);
    let mut f = form(None);
    assert_eq!(f.input(MenuInput::Key(MenuKey::Escape)), FormStep::Closed);
    assert!(f.take_sent().is_empty());
}

#[test]
fn rom_form_is_centred_at_two_window_sizes() {
    let Ok(rom) = std::env::var("CA_ROM") else {
        return;
    };
    let rom = std::path::Path::new(&rom);
    for (w, h) in SIZES {
        let mut f = ReportForm::open(Some(rom), None, (w, h), &["Bot1".to_string()], "Me", None);
        let items = f.items();
        let mut hud = Hud::from_rom(rom, w, h, None).expect("ROM HUD");
        hud.overlay = items;
        let list = hud.items(&HudState::default());
        let prims = resolve(&list, &mut hud.assets, w, h);
        // the window image (566 x 437 virtual) is drawn centred
        let want = [
            (1024.0 - 566.0) / 2.0 * w as f32 / 1024.0,
            (768.0 - 437.0) / 2.0 * h as f32 / 768.0,
        ];
        let found = prims.iter().any(|p| match p {
            Prim::Image { rect, .. } => {
                (rect[0] - want[0]).abs() <= 1.5 && (rect[1] - want[1]).abs() <= 1.5
            }
            _ => false,
        });
        assert!(found, "report window at {want:?} on {w}x{h}");
    }
}
