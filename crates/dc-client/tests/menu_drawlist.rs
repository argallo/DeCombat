//! The front end's structural check ([`dc_client::menu::drawlist`]): planted defects (a 3 px text shift, a font
//! swap, a missing button, a text under a later opaque image) are caught on synthetic rows; with `CA_ROM` set every
//! screen of the ROM front end has no record-rule mismatch (position, alignment, font index, font size, normal
//! colour, draw order, double draw, font index without a record) at 1024 x 768 and at 2560 x 1440. The server /
//! channel bars note what their dynamic texts ask for (the 2009 bar rule): a name drawn off its key position, a
//! centred state text or a wrong size is caught like a record's (planted defects), and the ROM bars pass.
// SPEC: FMT_CRES_UI.md §6.3 (record schemas, alignment codes), §7.1 (font records)
// EVIDENCE: cshell+0x16b770 (bar children at the bar plus their key positions), cshell+0x16b680 (the bar never draws
// its index text), cshell+0x16b140 (name / state: size 0xe, alignment 0x41)

use dc_client::hud::items::DrawItem;
use dc_client::hud::layout::TextStyle;
use dc_client::menu::catalog::Catalog;
use dc_client::menu::cursor;
use dc_client::menu::drawlist::{check, DynNotes, FontTable, Origin, RecordWant, TraceRow};
use dc_client::menu::popup::{BoxTag, Popup};
use dc_client::menu::servers::{draw_bar, BarRow, BarStyle};
use dc_client::menu::sheet::{Caption, Look, Part, Sheet};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_ui_data::pages::{Align, ALIGN_SECOND_CENTRE};
use dc_ui_data::{PageTag, WidgetKind};

fn part(kind: WidgetKind, pos: [i32; 2], look: Look) -> Part {
    Part {
        page: PageTag::Page5,
        kind,
        layer: -1,
        id: 1,
        command: 0,
        pos,
        size: [60, 20],
        look,
        parent: None,
        shown: true,
        enabled: true,
        text: String::new(),
        checked: false,
        selected: false,
        masked: false,
        hover: false,
        down: false,
        focus: false,
        hit: None,
        text_at: [0, 0],
        rec: 0,
        wrap: 0,
        wrap_pitch: 0,
        wrap_max: 0,
        colour_set: false,
        placed: false,
    }
}

fn centre() -> Align {
    Align {
        first: 4,
        second: ALIGN_SECOND_CENTRE,
    }
}

fn want(pos: [i32; 2], font: i32) -> RecordWant {
    RecordWant {
        page: 5,
        rec: 0,
        kind: WidgetKind::Text,
        layer: -1,
        id: 1,
        pos,
        size: [60, 20],
        text_at: Some([pos[0] as f32, pos[1] as f32]),
        text_anchor: Some([0.5, 0.5]),
        font: Some(font),
        font_size: Some(14),
        colour: Some(0xffee_eeee),
        rect: Some([0, 0, 60, 20]),
        align: Some(centre()),
        rom_text: true,
        colours: None,
    }
}

fn label(at: [f32; 2], font: i32) -> DrawItem {
    DrawItem::Label {
        style: TextStyle {
            font,
            colour: [0xee, 0xee, 0xee, 0xff],
            size: Some(14),
        },
        pos: at,
        text: "Title".into(),
        anchor: [0.5, 0.5],
    }
}

fn text_row(seq: usize, at: [f32; 2], font: i32) -> TraceRow {
    TraceRow {
        seq,
        origin: Origin::Part { part: 0, piece: 0 },
        item: label(at, font),
        rect: [at[0] - 15.0, at[1] - 7.0, 30.0, 14.0],
        font_record: None,
        font_own_face: None,
        want: Some(want([100, 50], 3)),
        note: None,
        frame_want: None,
    }
}

fn text_sheet() -> Sheet {
    Sheet {
        modal: None,
        modal_free: None,
        tiles: None,
        parts: vec![part(
            WidgetKind::Text,
            [100, 50],
            Look::Text {
                align: centre(),
                font: 3,
                size: 14,
                colour: 0xffee_eeee,
            },
        )],
    }
}

fn classes(rows: &[TraceRow], sheet: &Sheet) -> Vec<&'static str> {
    let mut cat = Catalog::fallback(None, 1024, 768);
    check(rows, sheet, &FontTable::default(), &mut cat)
        .into_iter()
        .map(|m| m.class)
        .filter(|c| *c != "CURSOR")
        .collect()
}

#[test]
fn a_text_on_its_record_passes() {
    let sheet = text_sheet();
    assert!(classes(&[text_row(1, [100.0, 50.0], 3)], &sheet).is_empty());
}

#[test]
fn a_three_pixel_shift_is_a_position_mismatch() {
    let sheet = text_sheet();
    let c = classes(&[text_row(1, [103.0, 50.0], 3)], &sheet);
    assert_eq!(c, vec!["POS"]);
    // within one pixel is a pass
    assert!(classes(&[text_row(1, [101.0, 49.0], 3)], &sheet).is_empty());
}

#[test]
fn a_font_swap_is_a_font_mismatch() {
    let sheet = text_sheet();
    assert_eq!(
        classes(&[text_row(1, [100.0, 50.0], 9)], &sheet),
        vec!["FONT"]
    );
}

#[test]
fn a_text_drawn_twice_is_a_double_draw() {
    let sheet = text_sheet();
    let rows = [text_row(1, [100.0, 50.0], 3), text_row(2, [100.0, 50.0], 3)];
    let c = classes(&rows, &sheet);
    assert!(c.contains(&"DUP"), "{c:?}");
}

#[test]
fn a_shown_button_that_draws_nothing_is_missing() {
    let sheet = Sheet {
        modal: None,
        modal_free: None,
        tiles: None,
        parts: vec![part(
            WidgetKind::Button,
            [10, 10],
            Look::Button {
                prefix: "x\\button_".into(),
                caption: Some(Caption {
                    string_id: 1,
                    at: [0, 0],
                    align: centre(),
                    font: 3,
                    size: 14,
                    colours: [0xffff_ffff; 4],
                }),
                frame: None,
            },
        )],
    };
    assert_eq!(classes(&[], &sheet), vec!["MISSING"]);
    // a hidden one is not
    let mut hidden = sheet.clone();
    hidden.parts[0].shown = false;
    assert!(classes(&[], &hidden).is_empty());
}

#[test]
fn no_cursor_image_is_reported() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let m = check(&[], &Sheet::default(), &FontTable::default(), &mut cat);
    assert_eq!(m.len(), 1);
    assert_eq!(m[0].class, "CURSOR");
}

/// The record-rule classes: everything the layout records decide.
const RECORD_RULES: [&str; 7] = ["POS", "ALIGN", "FONT", "FSIZE", "DUP", "ORDER", "RANGE"];

#[test]
fn rom_screens_follow_their_records() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    for (w, h) in [(1024, 768), (2560, 1440)] {
        let mut menu = Menu::new(MenuOptions {
            auth: "127.0.0.1:1".parse().unwrap(),
            user: "tester".into(),
            link: dc_client::menu::link::LinkOptions::new("drawlist-test"),
            rom: Some(rom.clone()),
            font: None,
            width: w,
            height: h,
        });
        menu.detach_link();
        let fonts = FontTable::from_catalog(&menu.cat);
        for g in [
            Go::Login,
            Go::Servers,
            Go::Channels,
            Go::Lobby,
            Go::Create,
            Go::Room,
        ] {
            menu.open(g);
            let rows = menu.draw_trace();
            assert!(rows.len() > 1, "{g:?}: nothing drawn");
            let sheet = menu.trace_sheet().clone();
            let bad: Vec<_> = check(&rows, &sheet, &fonts, &mut menu.cat)
                .into_iter()
                .filter(|m| RECORD_RULES.contains(&m.class))
                .collect();
            assert!(bad.is_empty(), "{w}x{h} {g:?}: {bad:#?}");
            // L1: the ROM cursor image is the last item, at the default pointer, and the check is quiet about it
            let last = rows.last().unwrap();
            assert_eq!(last.origin, Origin::Cursor, "{w}x{h} {g:?}");
            let cursor_rows = check(&rows, &sheet, &fonts, &mut menu.cat)
                .into_iter()
                .filter(|m| m.class == "CURSOR")
                .count();
            assert_eq!(cursor_rows, 0, "{w}x{h} {g:?}");
            let DrawItem::Image { dst, .. } = &last.item else {
                panic!("cursor is an image");
            };
            assert_eq!([dst[0], dst[1]], cursor::DEFAULT_POINTER);
        }
    }
}

/// The items control `(layer, id)` of `kind` drew.
fn drawn<'a>(
    rows: &'a [TraceRow],
    sheet: &Sheet,
    layer: i32,
    id: i32,
    kind: WidgetKind,
) -> Vec<&'a DrawItem> {
    rows.iter()
        .filter(|r| match r.origin {
            Origin::Part { part, .. } => {
                let p = &sheet.parts[part];
                (p.layer, p.id, p.kind) == (layer, id, kind)
            }
            _ => false,
        })
        .map(|r| &r.item)
        .collect()
}

fn label_text(item: &DrawItem) -> &str {
    match item {
        DrawItem::Label { text, .. } => text,
        _ => "",
    }
}

/// CLI-476..479: the room creation box keeps the lobby's character info filled and hides the password frame
/// until the check box is ticked; the waiting room shows the room number with its dot, the weapon limit and the
/// map description on several lines, one line height apart.
#[test]
fn rom_create_and_room_values_are_filled() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: dc_client::menu::link::LinkOptions::new("drawlist-test"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    menu.detach_link();
    menu.data.profile.nick = "tester".into();
    menu.data.profile.gp = 1234;
    menu.open(Go::Create);
    let rows = menu.draw_trace();
    let sheet = menu.trace_sheet().clone();
    // character info (window 7): name, GP with its separator, clan line
    let name = drawn(&rows, &sheet, 7, 3, WidgetKind::Text);
    assert_eq!(
        name.iter().map(|i| label_text(i)).collect::<Vec<_>>(),
        ["tester"]
    );
    let gp = drawn(&rows, &sheet, 7, 13, WidgetKind::Text);
    assert_eq!(
        gp.iter().map(|i| label_text(i)).collect::<Vec<_>>(),
        ["1,234"]
    );
    assert_eq!(drawn(&rows, &sheet, 7, 7, WidgetKind::Text).len(), 1);
    // the password frame (1, 2) and box (1, 10) are not drawn before the check
    assert!(drawn(&rows, &sheet, 1, 2, WidgetKind::Image).is_empty());
    assert!(drawn(&rows, &sheet, 1, 10, WidgetKind::EditBox).is_empty());
    // the player count of a team mode row reads as two halves
    let players = drawn(&rows, &sheet, 1, 16, WidgetKind::Text);
    assert!(
        players.iter().all(|i| label_text(i).contains(" vs ")),
        "{players:?}"
    );

    menu.data.room.number = 0;
    menu.data.room.map = 2;
    menu.data.room.mode = 8;
    menu.data.room.max_players = 16;
    menu.data.room.goal = 7;
    menu.data.room.weapons = 0;
    menu.open(Go::Room);
    let rows = menu.draw_trace();
    let sheet = menu.trace_sheet().clone();
    let number = drawn(&rows, &sheet, 5, 9, WidgetKind::Text);
    assert_eq!(
        number.iter().map(|i| label_text(i)).collect::<Vec<_>>(),
        ["001."]
    );
    let players = drawn(&rows, &sheet, 6, 19, WidgetKind::Text);
    assert_eq!(
        players.iter().map(|i| label_text(i)).collect::<Vec<_>>(),
        ["8 vs 8"]
    );
    let arms = drawn(&rows, &sheet, 6, 21, WidgetKind::Text);
    assert!(
        arms.len() == 1 && !label_text(arms[0]).is_empty(),
        "{arms:?}"
    );
    for (id, what) in [(14, "mode"), (17, "goal")] {
        let t = drawn(&rows, &sheet, 6, id, WidgetKind::Text);
        assert!(
            t.len() == 1 && !label_text(t[0]).is_empty(),
            "{what}: {t:?}"
        );
    }
    // the description: several lines, each inside the wrap width, one below the other
    let lines = drawn(&rows, &sheet, 6, 11, WidgetKind::Text);
    assert!(lines.len() >= 2, "{lines:?}");
    let ys: Vec<f32> = lines
        .iter()
        .map(|i| match i {
            DrawItem::Label { pos, .. } => pos[1],
            _ => f32::NAN,
        })
        .collect();
    assert!(ys.windows(2).all(|w| w[1] > w[0]), "{ys:?}");
    let widths: Vec<f32> = rows
        .iter()
        .filter(|r| lines.iter().any(|l| std::ptr::eq(*l, &r.item)))
        .map(|r| r.rect[2])
        .collect();
    assert!(
        widths
            .iter()
            .all(|w| *w <= dc_client::menu::roomtext::DESCRIPTION_WRAP as f32),
        "{widths:?}"
    );
}

/// A tiny 24-bit PCX made here: `w` x `h`, every texel black except a white diagonal, each plane line run-length
/// coded (the black stretches as runs, the white texel as a run of 1 since 255 >= 0xC0).
fn synthetic_pcx(w: u16, h: u16) -> Vec<u8> {
    let mut f = vec![0u8; 128];
    f[0] = 0x0a;
    f[1] = 5;
    f[2] = 1;
    f[3] = 8;
    f[8..10].copy_from_slice(&(w - 1).to_le_bytes());
    f[10..12].copy_from_slice(&(h - 1).to_le_bytes());
    f[65] = 3;
    f[66..68].copy_from_slice(&w.to_le_bytes());
    for y in 0..h {
        for _plane in 0..3 {
            for x in 0..w {
                if x == y {
                    f.extend_from_slice(&[0xc1, 255]);
                } else {
                    f.push(0);
                }
            }
        }
    }
    f
}

#[test]
fn a_synthetic_pcx_decodes_and_keys_out_black() {
    let img = dc_tex::decode_pcx(&synthetic_pcx(4, 3)).expect("decodes");
    assert_eq!((img.width, img.height), (4, 3));
    let tex = cursor::keyed(img);
    for y in 0..3u32 {
        for x in 0..4u32 {
            let o = ((y * 4 + x) * 4) as usize;
            let want = if x == y {
                [255, 255, 255, 255]
            } else {
                [0, 0, 0, 0]
            };
            assert_eq!(tex.rgba[o..o + 4], want, "texel {x},{y}");
        }
    }
}

#[test]
fn the_cursor_is_the_last_draw_item_at_the_pointer() {
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: dc_client::menu::link::LinkOptions::new("drawlist-cursor"),
        rom: None,
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 2048,
        height: 1536,
    });
    menu.detach_link();
    // no image: no cursor item (the window keeps the system cursor)
    assert!(!menu.has_cursor_image());
    assert!(menu.cursor_item().is_none());
    let tex = cursor::keyed(dc_tex::decode_pcx(&synthetic_pcx(8, 8)).unwrap());
    menu.cat.assets.put(cursor::CURSOR_ENTRY, tex);
    assert!(menu.has_cursor_image());
    // the default pointer before any input, then the scripted one
    assert_eq!(menu.pointer(), cursor::DEFAULT_POINTER);
    menu.input(MenuInput::Move([300.0, 200.0]));
    for popup in [false, true] {
        if popup {
            menu.show_popup(Popup::ok("box".into(), BoxTag::None));
        }
        let rows = menu.draw_trace();
        let last = rows.last().unwrap();
        assert_eq!(last.origin, Origin::Cursor, "popup {popup}");
        assert_eq!(
            rows.iter().filter(|r| r.origin == Origin::Cursor).count(),
            1
        );
        // one texel per target pixel: 8 texels on a 2x target are 4 virtual pixels
        assert_eq!(last.rect, [300.0, 200.0, 4.0, 4.0]);
        let items = menu.items();
        assert!(cursor::is_cursor(items.last().unwrap()));
        let fonts = FontTable::default();
        let sheet = menu.trace_sheet().clone();
        assert!(!check(&rows, &sheet, &fonts, &mut menu.cat)
            .iter()
            .any(|m| m.class == "CURSOR"));
    }
    // the drawn cursor follows a press too, and hides with the window's pointer
    menu.input(MenuInput::Press([10.0, 20.0]));
    assert_eq!(menu.pointer(), [10.0, 20.0]);
    menu.set_pointer_shown(false);
    assert!(menu.cursor_item().is_none());
    assert!(!menu.items().iter().any(cursor::is_cursor));
}

#[test]
fn a_cursor_that_is_not_last_is_reported() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let cur = TraceRow {
        seq: 0,
        origin: Origin::Cursor,
        item: cursor::item([8, 8], [0.0, 0.0], (1024, 768)),
        rect: [0.0, 0.0, 8.0, 8.0],
        font_record: None,
        font_own_face: None,
        want: None,
        note: None,
        frame_want: None,
    };
    let fill = TraceRow {
        seq: 1,
        origin: Origin::Extra,
        item: DrawItem::Fill {
            rect: [0.0, 0.0, 1.0, 1.0],
            colour: [0, 0, 0, 255],
        },
        rect: [0.0, 0.0, 1.0, 1.0],
        font_record: None,
        font_own_face: None,
        want: None,
        note: None,
        frame_want: None,
    };
    let c = |rows: &[TraceRow], cat: &mut Catalog| {
        check(rows, &Sheet::default(), &FontTable::default(), cat)
            .into_iter()
            .filter(|m| m.class == "CURSOR")
            .count()
    };
    assert_eq!(c(&[fill.clone(), cur.clone()], &mut cat), 0);
    assert_eq!(c(&[cur, fill], &mut cat), 1);
}

#[test]
fn no_installed_face_is_one_line_not_one_per_text() {
    use dc_client::menu::drawlist::FontSlot;
    let fonts = FontTable {
        slots: vec![Some(FontSlot {
            own_file: false,
            height: 14,
            bold: true,
            face: "Some Face".into(),
            file: None,
        })],
        system_face: None,
    };
    let sheet = text_sheet();
    let mut row = text_row(1, [100.0, 50.0], 0);
    row.want = None;
    let mut far = row.clone();
    far.seq = 2;
    far.rect[1] += 100.0;
    let rows = [row, far];
    let mut cat = Catalog::fallback(None, 1024, 768);
    let c: Vec<_> = check(&rows, &sheet, &fonts, &mut cat)
        .into_iter()
        .map(|m| m.class)
        .filter(|c| *c != "CURSOR")
        .collect();
    assert_eq!(c, vec!["SYSFACE"]);
}

fn bar_style() -> BarStyle {
    BarStyle {
        bar: None,
        bar_over: None,
        size: [303, 26],
        state_img_size: [31, 15],
        state_icon_size: [25, 25],
        index_at: [25, 13],
        name_at: [32, 13],
        state_at: [226, 13],
        state_img_at: [189, 5],
        state_icon_at: [7, 3],
        load: [0xff20_7484; 4],
        name_full: 0xff50_5050,
        name_normal: 0xffaa_aaaa,
        name_selected: 0xfffe_ad00,
    }
}

/// The trace rows of one bar drawn by [`draw_bar`] at (221, 207) with its notes.
fn bar_rows(cat: &mut Catalog) -> Vec<TraceRow> {
    let mut out = Vec::new();
    let mut notes = DynNotes::new();
    let row = BarRow {
        index: 0,
        name: "Bar Name",
        step: Some(0),
        refused: false,
        closed: false,
        full: false,
        selected: false,
        hover: false,
        badge: None,
    };
    draw_bar(
        &bar_style(),
        cat,
        &row,
        [221, 207],
        &mut out,
        Some(&mut notes),
    );
    out.into_iter()
        .enumerate()
        .map(|(seq, item)| TraceRow {
            seq,
            origin: Origin::Extra,
            item,
            rect: [0.0; 4],
            font_record: None,
            font_own_face: None,
            want: None,
            note: notes
                .iter()
                .find(|(i, _)| *i == seq)
                .map(|(_, w)| w.clone()),
            frame_want: None,
        })
        .collect()
}

fn mismatches(rows: &[TraceRow], cat: &mut Catalog) -> Vec<(&'static str, String)> {
    check(rows, &Sheet::default(), &FontTable::default(), cat)
        .into_iter()
        .filter(|m| m.class != "CURSOR")
        .map(|m| (m.class, m.key))
        .collect()
}

fn label_mut(r: &mut TraceRow) -> (&mut [f32; 2], &mut [f32; 2], &mut TextStyle) {
    match &mut r.item {
        DrawItem::Label {
            pos, anchor, style, ..
        } => (pos, anchor, style),
        other => panic!("not a label: {other:?}"),
    }
}

#[test]
fn a_bar_draws_name_and_state_on_the_2009_rule_and_no_index() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let rows = bar_rows(&mut cat);
    let texts: Vec<_> = rows
        .iter()
        .filter(|r| matches!(r.item, DrawItem::Label { .. }))
        .collect();
    // name and state only: the 2009 bar never draws its index text
    assert_eq!(texts.len(), 2);
    assert!(texts.iter().all(|r| r.note.is_some()));
    let n = texts[0].note.as_ref().unwrap();
    assert_eq!(n.text_at, [253.0, 220.0]);
    assert_eq!(n.text_anchor, [0.0, 0.5]);
    assert_eq!(texts[1].note.as_ref().unwrap().text_at, [447.0, 220.0]);
    assert!(mismatches(&rows, &mut cat).is_empty());
}

#[test]
fn a_bar_name_off_its_key_position_is_a_position_mismatch() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let mut rows = bar_rows(&mut cat);
    let i = rows.iter().position(|r| r.note.is_some()).unwrap();
    // the old 18 px name shift
    label_mut(&mut rows[i]).0[0] += 18.0;
    let m = mismatches(&rows, &mut cat);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].0, "POS");
    assert!(m[0].1.starts_with("dyn/bar0/"), "{m:?}");
}

#[test]
fn a_centred_or_resized_bar_state_text_is_caught() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let mut rows = bar_rows(&mut cat);
    let i = rows.iter().rposition(|r| r.note.is_some()).unwrap();
    // the old state text: centred between the state position and the bar end, size 12
    let (pos, anchor, style) = label_mut(&mut rows[i]);
    pos[0] += 38.5;
    anchor[0] = 0.5;
    style.size = Some(12);
    let mut c: Vec<_> = mismatches(&rows, &mut cat)
        .into_iter()
        .map(|m| m.0)
        .collect();
    c.sort();
    assert_eq!(c, vec!["ALIGN", "FSIZE", "POS"]);
}

#[test]
fn rom_server_and_channel_bars_follow_the_bar_rule() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: dc_client::menu::link::LinkOptions::new("drawlist-test"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    menu.detach_link();
    menu.data.servers = (0..3)
        .map(|i| dc_botclient::lobby::ServerEntry {
            id: i + 1,
            name: format!("Server {}", i + 1),
            ip: "127.0.0.1".into(),
            port: 1,
            open: 1,
            capacity: 100,
            current: [10, 60, 97][i as usize],
            clan_only: 0,
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    menu.data.channels = (0..3)
        .map(|i| dc_client::menu::link::ChannelRow {
            id: i,
            name: format!("Channel {}", i + 1),
            open: 1,
            capacity: 100,
            current: [5, 50, 90][i as usize],
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    let fonts = FontTable::from_catalog(&menu.cat);
    for g in [Go::Servers, Go::Channels] {
        menu.open(g);
        let rows = menu.draw_trace();
        let noted = rows.iter().filter(|r| r.note.is_some()).count();
        assert_eq!(noted, 6, "{g:?}: two noted texts per bar");
        let sheet = menu.trace_sheet().clone();
        let bad: Vec<_> = check(&rows, &sheet, &fonts, &mut menu.cat)
            .into_iter()
            .filter(|m| RECORD_RULES.contains(&m.class))
            .collect();
        assert!(bad.is_empty(), "{g:?}: {bad:#?}");
    }
}

// ---- w2-gaterows: dynamic rows of the lobby, the waiting room and the result screen; the run-time colour allow
// list; the cursor over a text; the resolution-independent description wrap; the room creation box start values

use dc_botclient::lobby::RoomMember;
use dc_client::menu::link::{RoomRow, UserRow};
use dc_client::menu::lobby::{
    draw_rooms, draw_user_flags, draw_user_marks, draw_users, room_list_number, RowStyle,
};

fn row_style() -> RowStyle {
    RowStyle {
        bar: None,
        size: [690, 31],
        lock: None,
        lock_size: [0, 0],
        lock_at: [0, 0],
        index_at: [30, 15],
        name_at: [88, 15],
        map_at: [325, 15],
        mode_at: [443, 15],
        members_at: [590, 15],
        state_at: [672, 15],
        user_bar: None,
        user_size: [212, 31],
        user_badge: None,
        user_badge_size: [0, 0],
        user_bar_at: [0, 0],
        user_name_at: [37, 15],
        user_badge_at: [0, 0],
        user_mark_at: [0, 0],
        user_flag_at: [0, 0],
        flag_size: [0, 0],
    }
}

fn list_part(layer: i32, id: i32, pos: [i32; 2]) -> Part {
    let mut p = part(
        WidgetKind::List,
        pos,
        Look::List {
            height: 0,
            spacing: 0,
            scroll: false,
            scroll_at: [0, 0],
            scroll_len: 0,
            bar: None,
        },
    );
    p.layer = layer;
    p.id = id;
    p
}

/// Trace rows of items drawn by a screen's own code, with its notes.
fn extra_rows(out: Vec<DrawItem>, notes: &DynNotes) -> Vec<TraceRow> {
    out.into_iter()
        .enumerate()
        .map(|(seq, item)| TraceRow {
            seq,
            origin: Origin::Extra,
            item,
            rect: [0.0; 4],
            font_record: None,
            font_own_face: None,
            want: None,
            note: notes
                .iter()
                .find(|(i, _)| *i == seq)
                .map(|(_, w)| w.clone()),
            frame_want: None,
        })
        .collect()
}

fn room_rows(cat: &mut Catalog) -> Vec<TraceRow> {
    let rooms = [
        RoomRow {
            number: 0,
            title: "first".into(),
            map: 0,
            mode: 1,
            players: 1,
            max_players: 16,
            ..RoomRow::default()
        },
        RoomRow {
            number: 12,
            title: "second".into(),
            locked: true,
            state: 2,
            players: 3,
            max_players: 8,
            ..RoomRow::default()
        },
    ];
    let list = list_part(5, 13, [16, 162]);
    let mut out = Vec::new();
    let mut notes = DynNotes::new();
    draw_rooms(
        &row_style(),
        cat,
        &list,
        &rooms,
        Some(0),
        None,
        &mut out,
        Some(&mut notes),
    );
    extra_rows(out, &notes)
}

#[test]
fn the_room_list_number_is_three_digits_of_the_number_plus_one_without_a_dot() {
    assert_eq!(room_list_number(0), "001");
    assert_eq!(room_list_number(12), "013");
    assert_eq!(room_list_number(998), "999");
}

#[test]
fn room_rows_draw_their_2009_rule() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let rows = room_rows(&mut cat);
    let texts: Vec<_> = rows
        .iter()
        .filter(|r| matches!(r.item, DrawItem::Label { .. }))
        .collect();
    // six texts per row, each noted
    assert_eq!(texts.len(), 12);
    assert!(texts.iter().all(|r| r.note.is_some()));
    assert_eq!(label_text(&texts[0].item), "001");
    assert_eq!(label_text(&texts[6].item), "013");
    assert_eq!(label_text(&texts[4].item), "1 / 16");
    let n = texts[0].note.as_ref().unwrap();
    assert_eq!(n.key, "dyn/room0/65fb375b13800675");
    assert_eq!(n.text_at, [46.0, 177.0]);
    assert_eq!(n.text_anchor, [0.5, 0.5]);
    assert_eq!((n.font, n.font_size), (Some(9), Some(14)));
    let title = texts[1].note.as_ref().unwrap();
    assert_eq!(title.text_anchor, [0.0, 0.5]);
    assert_eq!(title.font, Some(3));
    assert!(mismatches(&rows, &mut cat).is_empty());
}

#[test]
fn a_room_row_drawn_in_the_old_font_or_left_aligned_is_caught() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let mut rows = room_rows(&mut cat);
    let noted: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].note.is_some())
        .collect();
    // the number in the old font 14, the map text left-aligned
    label_mut(&mut rows[noted[0]]).2.font = 14;
    label_mut(&mut rows[noted[2]]).1[0] = 0.0;
    let m = mismatches(&rows, &mut cat);
    assert_eq!(
        m,
        vec![
            ("FONT", "dyn/room0/65fb375b13800675".to_string()),
            ("ALIGN", "dyn/room0/6a415ec0f936cf60".to_string()),
        ]
    );
}

#[test]
fn user_rows_draw_their_2009_rule() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let sheet = Sheet {
        modal: None,
        modal_free: None,
        tiles: None,
        parts: vec![list_part(6, 1, [772, 110])],
    };
    let data = dc_client::menu::MenuData {
        users: (0..3)
            .map(|i| UserRow {
                account: i,
                id: i,
                name: format!("user{i}"),
                exp: 0,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    };
    let mut out = Vec::new();
    let mut notes = DynNotes::new();
    draw_users(
        &row_style(),
        &sheet,
        &data,
        None,
        None,
        &|_| None,
        &mut out,
        Some(&mut notes),
    );
    let mut rows = extra_rows(out, &notes);
    let noted: Vec<usize> = (0..rows.len())
        .filter(|&i| rows[i].note.is_some())
        .collect();
    assert_eq!(noted.len(), 3);
    let n = rows[noted[1]].note.clone().unwrap();
    assert_eq!(n.text_at, [809.0, 156.0]);
    assert_eq!((n.font, n.font_size), (Some(3), Some(13)));
    assert!(mismatches(&rows, &mut cat).is_empty());
    // planted: a 4 px drop
    label_mut(&mut rows[noted[1]]).0[1] += 4.0;
    let m = mismatches(&rows, &mut cat);
    assert_eq!(m.len(), 1, "{m:?}");
    assert_eq!(m[0].0, "POS");
    assert!(m[0].1.starts_with("dyn/user1/"), "{m:?}");
}

fn rom_menu(rom: std::path::PathBuf, w: u32, h: u32) -> Menu {
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: dc_client::menu::link::LinkOptions::new("drawlist-test"),
        rom: Some(rom),
        font: None,
        width: w,
        height: h,
    });
    menu.detach_link();
    let d = &mut menu.data;
    d.profile.my_id = 1;
    d.rooms = vec![
        RoomRow {
            number: 0,
            title: "room a".into(),
            map: 2,
            mode: 0,
            players: 2,
            max_players: 8,
            ..RoomRow::default()
        },
        RoomRow {
            number: 1,
            title: "room b".into(),
            locked: true,
            mode: 1,
            players: 6,
            max_players: 16,
            state: 2,
            ..RoomRow::default()
        },
    ];
    d.users = (1..=2)
        .map(|i| UserRow {
            account: i,
            id: i,
            name: format!("user{i}"),
            exp: 0,
            ..Default::default()
        })
        .collect();
    d.room.host_id = 1;
    d.room.mode = 0;
    d.room.max_players = 8;
    for (id, team, state) in [(1u32, 1, 0), (2, 2, 1), (3, 1, 3)] {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: format!("member{id}"),
                team,
                state,
                exp: 0,
            },
        );
    }
    d.result = Some(dc_client::matchexit::MatchResult::sample());
    menu
}

/// Every dynamic row of the lobby (rooms, users), the waiting room (members) and the result screen (rows) carries
/// its 2009 rule and keeps it at both sizes; a row text resized after the fact (planted) is caught on its key.
#[test]
fn rom_dynamic_rows_follow_their_2009_rules() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    for (w, h) in [(1024, 768), (2560, 1440)] {
        let mut menu = rom_menu(rom.clone(), w, h);
        let fonts = FontTable::from_catalog(&menu.cat);
        // lobby: 6 texts per room, 1 per user; waiting room: a name per member, a state for the two with one;
        // result: place + 5 columns per row
        for (g, want, prefix) in [
            (Go::Lobby, 2 * 6 + 2, "dyn/room"),
            (Go::Room, 3 + 2, "dyn/member"),
            (Go::Result, 4 * 6, "dyn/result"),
        ] {
            menu.open(g);
            let mut rows = menu.draw_trace();
            let noted: Vec<usize> = (0..rows.len())
                .filter(|&i| rows[i].note.is_some())
                .collect();
            assert_eq!(noted.len(), want, "{w}x{h} {g:?}");
            let sheet = menu.trace_sheet().clone();
            let bad: Vec<_> = check(&rows, &sheet, &fonts, &mut menu.cat)
                .into_iter()
                .filter(|m| RECORD_RULES.contains(&m.class))
                .collect();
            assert!(bad.is_empty(), "{w}x{h} {g:?}: {bad:#?}");
            // planted: the first noted text two sizes up
            let i = noted[0];
            let st = label_mut(&mut rows[i]).2;
            st.size = st.size.map(|s| s + 2);
            let bad: Vec<_> = check(&rows, &sheet, &fonts, &mut menu.cat)
                .into_iter()
                .filter(|m| RECORD_RULES.contains(&m.class))
                .map(|m| (m.class, m.key))
                .collect();
            assert_eq!(bad.len(), 1, "{w}x{h} {g:?}: {bad:?}");
            assert_eq!(bad[0].0, "FSIZE");
            assert!(bad[0].1.starts_with(prefix), "{bad:?}");
        }
        // the lobby's numbers: three digits of the number + 1, no dot
        menu.open(Go::Lobby);
        let rows = menu.draw_trace();
        let firsts: Vec<_> = rows
            .iter()
            .filter(|r| {
                r.note
                    .as_ref()
                    .is_some_and(|n| n.key.ends_with("/65fb375b13800675"))
            })
            .map(|r| label_text(&r.item).to_string())
            .collect();
        assert_eq!(firsts, ["001", "002"]);
    }
}

/// CLI-1887: a name colour item's colour (the ROM colour table's entry of the record's number) reaches the room
/// list's user rows (0x371 +0x3a), the waiting room's member rows (0x38b +0xac) and the Character Info panel's own name
/// (0x303 +0xe0); rows without a number keep their own colours.
#[test]
fn rom_a_coloured_name_reaches_the_room_list_and_the_member_rows() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let word = dc_client::menu::charinfo::table_colour(Some(&rom), 2)
        .expect("the ROM colour table has a second entry");
    let rgba = |c: u32| [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8];
    let colour_of = |rows: &[TraceRow], want: &str| {
        rows.iter()
            .find_map(|r| match &r.item {
                DrawItem::Label { text, style, .. } if text == want => Some(style.colour),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{want} drawn"))
    };
    let mut menu = rom_menu(rom, 1024, 768);
    menu.data.profile.nick = "tester".into();
    menu.data.users[1].name_colour = 2;
    menu.data.member_colour.insert(2, (2, 0));
    menu.data.char_extras.name_colour = 2;
    menu.open(Go::Lobby);
    let rows = menu.draw_trace();
    assert_eq!(colour_of(&rows, "user2"), rgba(word), "a coloured user row");
    assert_eq!(
        colour_of(&rows, "user1"),
        rgba(dc_client::menu::lobby::OWN_COLOUR),
        "the own row without a number"
    );
    assert_eq!(
        colour_of(&rows, "tester"),
        rgba(word),
        "the own name on the panel"
    );
    menu.open(Go::Room);
    let rows = menu.draw_trace();
    assert_eq!(
        colour_of(&rows, "member2"),
        rgba(word),
        "a coloured member row"
    );
    assert_eq!(
        colour_of(&rows, "member1"),
        rgba(dc_client::menu::room::OWN_NAME_COLOUR),
        "the own member row without a number"
    );
}

/// The baseline's run-time colour keys (CLI-537).
fn run_colour_keys() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("golden")
        .join("uidiff_baseline.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    v["run_colour"]
        .as_array()
        .expect("run_colour list")
        .iter()
        .map(|k| k.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn a_colour_set_at_run_time_is_still_a_colour_mismatch() {
    let mut sheet = text_sheet();
    let mut row = text_row(1, [100.0, 50.0], 3);
    if let DrawItem::Label { style, .. } = &mut row.item {
        style.colour = [0xb4, 0xb4, 0xb4, 0xff];
    }
    assert_eq!(classes(std::slice::from_ref(&row), &sheet), vec!["COLOUR"]);
    // the screen's own colour is no exemption by itself: the gate's per-key list decides
    sheet.parts[0].colour_set = true;
    assert_eq!(classes(&[row], &sheet), vec!["COLOUR"]);
}

/// The ROM lobby's COLOUR rows are exactly the documented run-time keys of the baseline; a run-time colour on any
/// other text (planted) is a COLOUR row outside that list, which the gate counts.
#[test]
fn rom_run_time_colours_are_the_listed_keys_only() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let allowed = run_colour_keys();
    assert_eq!(allowed.len(), 4);
    let mut menu = rom_menu(rom, 1024, 768);
    menu.data.profile.nick = "tester".into();
    let fonts = FontTable::from_catalog(&menu.cat);
    for g in [Go::Lobby, Go::Create] {
        menu.open(g);
        let mut rows = menu.draw_trace();
        let mut sheet = menu.trace_sheet().clone();
        let colour_keys = |rows: &[TraceRow], sheet: &Sheet, cat: &mut Catalog| {
            let mut k: Vec<String> = check(rows, sheet, &fonts, cat)
                .into_iter()
                .filter(|m| m.class == "COLOUR")
                .map(|m| m.key)
                .collect();
            k.sort();
            k
        };
        assert_eq!(colour_keys(&rows, &sheet, &mut menu.cat), allowed, "{g:?}");
        // planted: the character's name (window 7 text 3) recoloured at run time
        let i = rows
            .iter()
            .position(|r| match r.origin {
                Origin::Part { part, .. } => {
                    let p = &sheet.parts[part];
                    (p.layer, p.id, p.kind) == (7, 3, WidgetKind::Text)
                }
                _ => false,
            })
            .expect("name drawn");
        let Origin::Part { part, .. } = rows[i].origin else {
            unreachable!()
        };
        sheet.parts[part].colour_set = true;
        label_mut(&mut rows[i]).2.colour = [1, 2, 3, 255];
        let keys = colour_keys(&rows, &sheet, &mut menu.cat);
        let counted: Vec<_> = keys.iter().filter(|k| !allowed.contains(k)).collect();
        assert_eq!(counted.len(), 1, "{g:?}: {keys:?}");
        assert!(counted[0].ends_with("/w7/id3"), "{counted:?}");
    }
}

/// The cursor drawn over a text is not a COVERED row (it covers whatever is under the pointer on purpose); the
/// same opaque image drawn by the screen is.
#[test]
fn the_cursor_over_a_text_is_not_covered_but_an_image_is() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    cat.assets.put(
        "opaque",
        dc_client::hud::compose::Tex {
            width: 8,
            height: 8,
            rgba: vec![255; 8 * 8 * 4],
        },
    );
    let mut text = text_row(0, [100.0, 50.0], 3);
    text.want = None;
    text.origin = Origin::Extra;
    let image = |origin: Origin| TraceRow {
        seq: 1,
        origin,
        item: DrawItem::Image {
            tex: "opaque".into(),
            src: [8.0, 8.0],
            dst: [80.0, 40.0, 40.0, 20.0],
            tint: [255; 4],
            turn: None,
        },
        rect: [80.0, 40.0, 40.0, 20.0],
        font_record: None,
        font_own_face: None,
        want: None,
        note: None,
        frame_want: None,
    };
    let covered = |rows: &[TraceRow], cat: &mut Catalog| {
        check(rows, &Sheet::default(), &FontTable::default(), cat)
            .into_iter()
            .filter(|m| m.class == "COVERED")
            .count()
    };
    assert_eq!(covered(&[text.clone(), image(Origin::Extra)], &mut cat), 1);
    assert_eq!(covered(&[text, image(Origin::Cursor)], &mut cat), 0);
}

/// CLI-540: the map description breaks into the same lines at every target size.
#[test]
fn rom_description_wraps_the_same_at_every_size() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let mut lines_at = Vec::new();
    for (w, h) in [(1024, 768), (2560, 1440), (1280, 1024)] {
        let mut menu = rom_menu(rom.clone(), w, h);
        menu.data.room.map = 2;
        menu.data.room.mode = 8;
        menu.open(Go::Room);
        let rows = menu.draw_trace();
        let sheet = menu.trace_sheet().clone();
        let lines: Vec<String> = drawn(&rows, &sheet, 6, 11, WidgetKind::Text)
            .iter()
            .map(|i| label_text(i).to_string())
            .collect();
        assert!(lines.len() >= 2, "{w}x{h}: {}", lines.len());
        lines_at.push(lines);
    }
    assert_eq!(lines_at[0], lines_at[1]);
    assert_eq!(lines_at[0], lines_at[2]);
}

/// CLI-541: the room creation box starts on mode row 8 when it is offered, with that row's recommended players;
/// CLI-542: the waiting room's goal of mode row 4 uses the code's own form.
#[test]
fn rom_create_box_starts_on_row_8_and_row_4_goal_uses_the_code_form() {
    let Some(rom) = std::env::var_os("CA_ROM").map(std::path::PathBuf::from) else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let mut menu = rom_menu(rom, 1024, 768);
    menu.data.profile.nick = "tester".into();
    menu.open(Go::Create);
    let rows = menu.draw_trace();
    let sheet = menu.trace_sheet().clone();
    let mode = drawn(&rows, &sheet, 1, 17, WidgetKind::Text);
    let players = drawn(&rows, &sheet, 1, 16, WidgetKind::Text);
    let start = dc_client::menu::create::START_MODE_ROW;
    let max = menu.cat.mode_players(start).map(|p| p.1).unwrap_or(16);
    let n = dc_client::menu::create::pick_players(menu.cat.mode_recommended_players(start), max);
    assert_eq!(
        mode.iter().map(|i| label_text(i)).collect::<Vec<_>>(),
        [menu.cat.mode_full_name(start).as_str()]
    );
    assert_eq!(
        players.iter().map(|i| label_text(i)).collect::<Vec<_>>(),
        [dc_client::menu::roomtext::player_count_label(&menu.cat, start, n).as_str()]
    );
    // OBSERVED R1_create.png: the 2009 box starts with 16 players (8 vs 8), the row's own recommendation
    assert_eq!(menu.cat.mode_recommended_players(start), Some(16));
    assert_eq!(n, 16);
    // row 4's goal: the integer and the code's points word, not the number alone
    assert!(menu.cat.paths.goal_points.is_some());
    let goal = dc_client::menu::roomtext::goal_label(&menu.cat, 4, 7);
    assert!(goal.starts_with("7 ") && goal.len() > 2, "{goal:?}");
}

// ---- w2-uisound (M27): the open drop-down list is tagged in the draw list and checked against the 2009 rule
// EVIDENCE: cshell+0x33c350 / cshell+0x33c620 (rows and frame), cshell+0x163550 (3 px under the control)

/// The rows an open list adds to a trace (as `Menu::draw_trace` makes them): frame first with the rule's frame,
/// row texts noted with the rule's rows.
fn drop_rows(cat: &mut Catalog, d: &dc_client::menu::dropdown::Dropdown) -> Vec<TraceRow> {
    use dc_client::menu::dropdown::{rule_frame, rule_row, DropArt};
    let mut out = Vec::new();
    let texts = d.draw(cat, &DropArt::default(), &mut out);
    out.into_iter()
        .enumerate()
        .map(|(seq, item)| {
            let rect = match &item {
                DrawItem::Fill { rect, .. } => *rect,
                DrawItem::Image { dst, .. } => *dst,
                _ => [0.0; 4],
            };
            TraceRow {
                seq,
                origin: Origin::Dropdown {
                    list: d.list,
                    piece: seq,
                },
                item,
                rect,
                font_record: None,
                font_own_face: None,
                want: None,
                note: texts
                    .iter()
                    .find(|(i, _)| *i == seq)
                    .map(|(_, k)| rule_row(d.list, d.anchor, *k, d.font_size)),
                frame_want: (seq == 0).then(|| rule_frame(d.anchor, d.shown().len(), d.font_size)),
            }
        })
        .collect()
}

fn goal_list() -> dc_client::menu::dropdown::Dropdown {
    dc_client::menu::dropdown::Dropdown::new(
        "goal",
        [770, 420, 150, 20],
        vec![("a".into(), 1), ("b".into(), 2), ("c".into(), 3)],
        14,
        false,
    )
}

#[test]
fn an_open_list_on_the_rule_has_no_mismatch() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let rows = drop_rows(&mut cat, &goal_list());
    assert_eq!(rows.iter().filter(|r| r.note.is_some()).count(), 3);
    assert_eq!(mismatches(&rows, &mut cat), vec![]);
}

#[test]
fn a_planted_list_frame_shift_is_a_drop_mismatch() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let mut rows = drop_rows(&mut cat, &goal_list());
    rows[0].rect[1] += 5.0;
    let bad = mismatches(&rows, &mut cat);
    assert_eq!(bad, vec![("DROP", "dropdown/goal".to_string())]);
}

#[test]
fn a_planted_list_row_off_its_place_is_a_position_mismatch() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let mut rows = drop_rows(&mut cat, &goal_list());
    let r = rows.iter_mut().find(|r| r.note.is_some()).unwrap();
    label_mut(r).0[1] += 4.0;
    let bad = mismatches(&rows, &mut cat);
    assert_eq!(bad, vec![("POS", "dyn/drop/goal/row0".to_string())]);
    // a left-aligned row is an alignment mismatch
    let mut rows = drop_rows(&mut cat, &goal_list());
    let r = rows.iter_mut().find(|r| r.note.is_some()).unwrap();
    label_mut(r).1[0] = 0.0;
    assert!(mismatches(&rows, &mut cat).contains(&("ALIGN", "dyn/drop/goal/row0".to_string())));
}

#[test]
fn a_planted_screen_item_over_the_list_or_a_list_without_rows_is_caught() {
    let mut cat = Catalog::fallback(None, 1024, 768);
    let mut rows = drop_rows(&mut cat, &goal_list());
    rows.push(TraceRow {
        seq: rows.len(),
        origin: Origin::Extra,
        item: DrawItem::Fill {
            rect: [0.0, 0.0, 4.0, 4.0],
            colour: [0, 0, 0, 255],
        },
        rect: [0.0, 0.0, 4.0, 4.0],
        font_record: None,
        font_own_face: None,
        want: None,
        note: None,
        frame_want: None,
    });
    assert_eq!(
        mismatches(&rows, &mut cat),
        vec![("DROP", "dynamic".to_string())]
    );
    let rows: Vec<TraceRow> = drop_rows(&mut cat, &goal_list())
        .into_iter()
        .filter(|r| r.note.is_none())
        .collect();
    assert_eq!(
        mismatches(&rows, &mut cat),
        vec![("DROP", "dropdown/goal".to_string())]
    );
}

#[test]
fn rom_open_setting_lists_are_tagged_and_follow_the_rule() {
    let Some(rom) = std::env::var_os("CA_ROM")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_dir())
    else {
        println!("skipped: CA_ROM unset");
        return;
    };
    use dc_client::menu::{create, room};
    let mut menu = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: dc_client::menu::link::LinkOptions::new("drop"),
        rom: Some(rom),
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    menu.detach_link();
    menu.data.profile.my_id = 1;
    menu.data.room.host_id = 1;
    menu.data.room.goal = 50;
    menu.data.masks = (0..8).map(|i| (i, 0xffff)).collect();
    let fonts = FontTable::from_catalog(&menu.cat);
    for (g, cmd, list) in [
        (Go::Room, room::CMD_MAP, "map"),
        (Go::Room, room::CMD_GOAL, "goal"),
        (Go::Room, room::CMD_ARMS, "arms"),
        (Go::Create, create::CMD_MODE_LIST, "create_mode"),
    ] {
        menu.open(g);
        let b = menu.sheet().button(cmd).expect("list button").rect();
        let c = [
            b[0] as f32 + b[2] as f32 / 2.0,
            b[1] as f32 + b[3] as f32 / 2.0,
        ];
        menu.input(MenuInput::Move(c));
        menu.input(MenuInput::Press(c));
        menu.input(MenuInput::Release(c));
        assert!(menu.dropdown().is_some(), "{list} open");
        let rows = menu.draw_trace();
        let tagged: Vec<&TraceRow> = rows
            .iter()
            .filter(|r| matches!(r.origin, Origin::Dropdown { list: l, .. } if l == list))
            .collect();
        assert!(tagged.len() >= 2, "{list}: frame and rows tagged");
        assert!(tagged[0].frame_want.is_some());
        // the list is the last thing under the cursor
        let last_screen = rows
            .iter()
            .rev()
            .find(|r| r.origin != Origin::Cursor)
            .unwrap();
        assert!(
            matches!(last_screen.origin, Origin::Dropdown { .. }),
            "{list} on top"
        );
        let json = dc_client::menu::drawlist::row_json("room_drop", tagged[0], Default::default());
        assert_eq!(json["origin"]["from"], "dropdown");
        assert_eq!(json["origin"]["list"], list);
        let sheet = menu.trace_sheet().clone();
        let bad: Vec<_> = check(&rows, &sheet, &fonts, &mut menu.cat)
            .into_iter()
            .filter(|m| m.class == "DROP" || m.key.starts_with("dyn/drop"))
            .collect();
        assert!(bad.is_empty(), "{list}: {bad:?}");
    }
}

/// CLI-940: a user row with a clan and both mark layers draws the mark (21 x 21 from the 51-texel art) at its row
/// plus UserClanMarkPos; a row without a clan or with a layer missing draws none.
#[test]
fn user_rows_draw_their_clan_mark() {
    let sheet = Sheet {
        modal: None,
        modal_free: None,
        tiles: None,
        parts: vec![list_part(6, 1, [772, 110])],
    };
    let user = |i: u32, clan, mark, ground| UserRow {
        account: i,
        id: i,
        name: format!("u{i}"),
        clan,
        mark,
        ground,
        ..Default::default()
    };
    let data = dc_client::menu::MenuData {
        users: vec![user(0, 5, 1, 1), user(1, 0, 1, 1), user(2, 5, 1, 0)],
        ..Default::default()
    };
    let marks = dc_client::menu::clan::ClanMarks {
        emblems: vec![None, Some("e1".into())],
        backgrounds: vec![None, Some("b1".into())],
        ..Default::default()
    };
    let mut style = row_style();
    style.user_mark_at = [180, 5];
    let mut out = Vec::new();
    draw_user_marks(&style, &sheet, &data, &marks, &mut out);
    assert_eq!(out.len(), 2, "background and emblem of the first row only");
    for (item, want) in out.iter().zip(["b1", "e1"]) {
        let DrawItem::Image { tex, src, dst, .. } = item else {
            panic!("image expected");
        };
        assert_eq!(tex, want);
        assert_eq!(*src, [51.0, 51.0]);
        assert_eq!(*dst, [772.0 + 180.0, 110.0 + 5.0, 21.0, 21.0]);
    }
}

/// CLI-1024: a user row with a nation draws that country's flag at its row plus NationalFlagPos, in FlagSize (the
/// texture's own size without one); nation 0 and a country without a flag draw none; the flag comes from the row's
/// own nation.
#[test]
fn user_rows_draw_their_nation_flag() {
    let sheet = Sheet {
        modal: None,
        modal_free: None,
        tiles: None,
        parts: vec![list_part(6, 1, [772, 110])],
    };
    let user = |i: u32, nation| UserRow {
        account: i,
        id: i,
        name: format!("u{i}"),
        nation,
        ..Default::default()
    };
    let data = dc_client::menu::MenuData {
        users: vec![user(0, 3), user(1, 0), user(2, 9), user(3, 4)],
        ..Default::default()
    };
    let mut style = row_style();
    style.user_flag_at = [150, 8];
    let flags = |n: u32| match n {
        3 => Some(("f3".to_string(), [24.0, 16.0])),
        4 => Some(("f4".to_string(), [24.0, 16.0])),
        _ => None,
    };
    let mut out = Vec::new();
    draw_user_flags(&style, &sheet, &data, &mut |n| flags(n), &mut out);
    assert_eq!(
        out.len(),
        2,
        "rows 0 and 3 only (nation 0 and an unknown country draw none)"
    );
    let want = [("f3", 0.0), ("f4", 93.0)];
    for (item, (name, dy)) in out.iter().zip(want) {
        let DrawItem::Image { tex, src, dst, .. } = item else {
            panic!("image expected");
        };
        assert_eq!(tex, name);
        assert_eq!(*src, [24.0, 16.0]);
        assert_eq!(*dst, [772.0 + 150.0, 110.0 + 8.0 + dy, 24.0, 16.0]);
    }
    // the layout's FlagSize wins over the texture size
    style.flag_size = [20, 14];
    let mut out = Vec::new();
    draw_user_flags(&style, &sheet, &data, &mut |n| flags(n), &mut out);
    let DrawItem::Image { dst, .. } = &out[0] else {
        panic!("image expected");
    };
    assert_eq!(dst[2..], [20.0, 14.0]);
}
