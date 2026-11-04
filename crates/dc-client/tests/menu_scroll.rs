//! lobby-scroll (CLI-700): the waiting room's map list and the channel list scroll by wheel, arrows and thumb
//! drag through the menu's own input path, with the user's ROM (`CA_ROM`; skipped without it).
//! With `DC_SCROLL_SHOTS=<dir>` the scrolled screens are written as PNGs (the Rust side of the reference stack comparison).

use dc_botclient::lobby::LobbyProfile;
use dc_client::menu::link::{ChannelRow, LinkOptions, RoomView};
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};
use dc_ui_data::WidgetKind;

fn menu() -> Option<Menu> {
    let rom = std::env::var_os("CA_ROM").map(std::path::PathBuf::from)?;
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "casey".into(),
        link: LinkOptions::new("menu-scroll"),
        rom: Some(rom),
        font: Some((dc_client::testkit::synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m.set_idle_rules(false);
    m.data.profile = LobbyProfile {
        nick: "casey".into(),
        char_no: 1,
        my_id: 1,
        chan_label: "Channel 1".into(),
        ..LobbyProfile::default()
    };
    Some(m)
}

fn shot(m: &mut Menu, name: &str) {
    let Some(dir) = std::env::var_os("DC_SCROLL_SHOTS") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let img = m.image();
    dc_render::FrameImage {
        width: img.width,
        height: img.height,
        rgba: img.rgba,
    }
    .save_png(&dir.join(format!("{name}.png")))
    .unwrap();
}

#[test]
fn the_map_list_scrolls_by_wheel_arrow_and_thumb() {
    let Some(mut m) = menu() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    // the lobby's offer: the ROM room table's map / mode masks (what dc-lobby-server pushes), the room in the mode
    // the reference client's auto room uses (Elimination Pro) on Snow Valley, as in its picture
    let rom = std::env::var_os("CA_ROM").unwrap();
    let room =
        dc_bute::GameRoomData::from_vfs(&dc_bute::RezVfs::new(std::path::Path::new(&rom))).unwrap();
    m.data.masks = room
        .map_mode_masks(false)
        .into_iter()
        .map(|(map, mask)| (map as u32, mask))
        .collect();
    let mode = (0..m.cat.mode_count() as i32)
        .find(|&i| m.cat.mode_full_name(i) == "Elimination Pro")
        .unwrap_or(0);
    let snow = (0..19)
        .find(|&i| m.cat.map_label(i) == "Snow Valley")
        .unwrap_or(0);
    m.data.room = RoomView {
        number: 0,
        title: "casey room".into(),
        host_id: 1,
        map: snow,
        mode,
        players: 1,
        max_players: 16,
        goal: 100,
        ..RoomView::default()
    };
    m.open(Go::Room);
    m.input(MenuInput::Command(dc_client::menu::room::CMD_MAP));
    let d = m.dropdown().expect("the map list opens").clone();
    let n = d.items.len();
    assert!(n > 5, "the map list holds {n} maps");
    assert_eq!(d.shown(), 0..5);
    // the wheel scrolls the open list wherever the pointer is: one row per turn, to the end
    m.input(MenuInput::Wheel([100.0, 100.0], -1));
    assert_eq!(m.dropdown().unwrap().shown(), 1..6);
    for _ in 0..n {
        m.input(MenuInput::Wheel([100.0, 100.0], -1));
    }
    let d = m.dropdown().unwrap().clone();
    assert_eq!(d.shown(), n - 5..n);
    let b = d.bar.clone().unwrap();
    let (len, off) = b.thumb();
    assert_eq!(off + len, b.track(), "the thumb at the track's end");
    shot(&mut m, "room_map_bottom");
    // a row of the scrolled list is picked by its index in the whole list
    let last = d.row_point(n - 1).unwrap();
    m.input(MenuInput::Move(last));
    assert_eq!(m.dropdown().unwrap().hover, Some(n - 1));
    // the up arrow steps back one row
    let up = b.arrow_point(-1);
    m.input(MenuInput::Press(up));
    m.input(MenuInput::Release(up));
    assert_eq!(m.dropdown().unwrap().shown(), n - 6..n - 1);
    // a press on the track's top drags the thumb there; the rows follow the pointer, the release ends it
    let top = [b.rect[0] as f32 + 7.0, (b.rect[1] + 12) as f32];
    m.input(MenuInput::Press(top));
    assert_eq!(m.dropdown().unwrap().shown(), 0..5);
    m.input(MenuInput::Move(b.drag_point(b.top() / 2)));
    let mid = m.dropdown().unwrap().shown();
    assert!(mid.start > 0 && mid.end < n, "{mid:?}");
    m.input(MenuInput::Release(b.drag_point(b.top() / 2)));
    m.input(MenuInput::Move([b.rect[0] as f32, 700.0]));
    assert_eq!(
        m.dropdown().unwrap().shown(),
        mid,
        "no drag after the release"
    );
    assert!(m.dropdown().is_some(), "scrolling does not close the list");
}

#[test]
fn the_channel_list_scrolls_under_the_pointer() {
    let Some(mut m) = menu() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    m.data.channels = (0..30)
        .map(|i| ChannelRow {
            id: i,
            name: format!("Channel {}", i + 1),
            open: 1,
            capacity: 100,
            current: 5,
            op_capacity: 0,
            op_current: 0,
        })
        .collect();
    m.open(Go::Channels);
    let _ = m.items();
    let li = m
        .sheet()
        .find(
            dc_client::menu::channels::WINDOW,
            dc_client::menu::channels::LIST,
            WidgetKind::List,
        )
        .unwrap();
    let list = m.sheet().parts[li].clone();
    let b = m
        .sheet()
        .list_bar(li)
        .expect("the channel list has its bar")
        .clone();
    assert!(b.top() > 0, "30 channels overflow the list");
    let inside = [list.pos[0] as f32 + 20.0, list.pos[1] as f32 + 20.0];
    // a turn away from the list does nothing
    m.input(MenuInput::Wheel([900.0, 700.0], -1));
    assert_eq!(m.sheet().list_bar(li).unwrap().value, 0);
    m.input(MenuInput::Wheel(inside, -3));
    assert_eq!(m.sheet().list_bar(li).unwrap().value, 3 * b.step);
    shot(&mut m, "channels_scrolled");
    m.input(MenuInput::Wheel(inside, 1));
    assert_eq!(m.sheet().list_bar(li).unwrap().value, 2 * b.step);
    // the down arrow, then a drag to the end
    let down = b.arrow_point(1);
    m.input(MenuInput::Press(down));
    m.input(MenuInput::Release(down));
    assert_eq!(m.sheet().list_bar(li).unwrap().value, 3 * b.step);
    let mid = [b.rect[0] as f32 + 7.0, (b.rect[1] + b.rect[3] / 2) as f32];
    m.input(MenuInput::Press(mid));
    m.input(MenuInput::Move([mid[0], 760.0]));
    m.input(MenuInput::Release([mid[0], 760.0]));
    assert_eq!(m.sheet().list_bar(li).unwrap().value, b.top());
    shot(&mut m, "channels_bottom");
    // the first row drawn is a later channel
    let first = (0..30)
        .find(|&i| dc_client::menu::sheet::list_row(&m.sheet().parts[li], 20, i, 0).is_some());
    assert!(first.is_some_and(|i| i > 10), "{first:?}");
}

/// The reference client's room offers the first 12 maps for Elimination Pro (its own list rule, not the lobby's masks): the
/// same 12 rows here, scrolled to the end by the wheel, show the reference stack's last five (Sand Hog .. Death Room).
#[test]
fn a_twelve_map_list_ends_on_its_last_five_rows() {
    let Some(mut m) = menu() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    m.data.masks = (0..12).map(|i| (i, u32::MAX)).collect();
    m.data.room = RoomView {
        number: 0,
        title: "casey room".into(),
        host_id: 1,
        map: 0,
        mode: 8,
        players: 1,
        max_players: 16,
        goal: 100,
        ..RoomView::default()
    };
    m.open(Go::Room);
    m.input(MenuInput::Command(dc_client::menu::room::CMD_MAP));
    shot(&mut m, "room_map_open12");
    for _ in 0..40 {
        m.input(MenuInput::Wheel([887.0, 600.0], -1));
    }
    let d = m.dropdown().unwrap().clone();
    assert_eq!(d.shown(), 7..12);
    assert_eq!(d.items[7], m.cat.map_label(7));
    shot(&mut m, "room_map_bottom12");
}

/// lobby-fixes10 item 1: the open list is the 2009 frame control (BOX_32_32 in nine parts: an opaque
/// middle, not the whole texture stretched) and the row under the pointer gets the hover bar (BOX_8_8 in nine parts)
/// from the frame's left + 2 to the scroll bar, as high as the row.
#[test]
fn the_open_list_has_an_opaque_frame_and_a_hover_bar() {
    use dc_client::hud::DrawItem;
    use dc_client::menu::drawlist::Origin;
    let Some(mut m) = menu() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    m.data.masks = (0..12).map(|i| (i, u32::MAX)).collect();
    m.data.room = RoomView {
        number: 0,
        title: "casey room".into(),
        host_id: 1,
        map: 0,
        mode: 8,
        players: 1,
        max_players: 16,
        goal: 100,
        ..RoomView::default()
    };
    m.open(Go::Room);
    m.input(MenuInput::Command(dc_client::menu::room::CMD_MAP));
    for _ in 0..40 {
        m.input(MenuInput::Wheel([887.0, 600.0], -1));
    }
    let d = m.dropdown().unwrap().clone();
    // the pointer on the second shown row
    let at = d.row_point(8).unwrap();
    m.input(MenuInput::Move(at));
    let rows = m.draw_trace();
    let drop: Vec<_> = rows
        .iter()
        .filter(|r| matches!(r.origin, Origin::Dropdown { .. }))
        .collect();
    let r = d.rect();
    // the middle part of the frame: inside the corners, opaque black texels
    let mid = drop
        .iter()
        .find_map(|t| match &t.item {
            DrawItem::Image { tex, dst, .. }
                if tex.contains("BOX_32_32") && dst[0] > r[0] as f32 && dst[1] > r[1] as f32 =>
            {
                Some((tex.clone(), *dst))
            }
            _ => None,
        })
        .expect("the frame's middle part");
    assert_eq!(
        mid.1,
        [
            (r[0] + 8) as f32,
            (r[1] + 8) as f32,
            (r[2] - 16) as f32,
            (r[3] - 16) as f32
        ]
    );
    let t = m.cat.assets.texture(&mid.0).unwrap();
    // DXT texels: 254 / 255
    assert!(t.rgba.chunks(4).all(|p| p[3] >= 250), "an opaque middle");
    // the hover bar's parts cover the hovered row
    let bar = d.bar.as_ref().unwrap();
    let want = [
        (r[0] + 2) as f32,
        at[1] - 7.0,
        (bar.rect[0] - r[0] - 2) as f32,
        14.0,
    ];
    let hl: Vec<[f32; 4]> = drop
        .iter()
        .filter_map(|t| match &t.item {
            DrawItem::Image { tex, dst, .. } if tex.contains("BOX_8_8") => Some(*dst),
            _ => None,
        })
        .collect();
    assert!(!hl.is_empty(), "a hover bar");
    let x0 = hl.iter().map(|d| d[0]).fold(f32::MAX, f32::min);
    let y0 = hl.iter().map(|d| d[1]).fold(f32::MAX, f32::min);
    let x1 = hl.iter().map(|d| d[0] + d[2]).fold(f32::MIN, f32::max);
    let y1 = hl.iter().map(|d| d[1] + d[3]).fold(f32::MIN, f32::max);
    assert_eq!([x0, y0, x1 - x0, y1 - y0], want);
    shot(&mut m, "room_map_hover12");
    // the pointer off the rows: no hover bar
    m.input(MenuInput::Move([10.0, 10.0]));
    assert!(!m
        .draw_trace()
        .iter()
        .any(|t| matches!(&t.item, DrawItem::Image { tex, .. } if tex.contains("BOX_8_8"))));
}

/// lobby-fixes10 item 3: the lobby chat list scrolls with its 2009 bar: it follows the newest line (40 lines: the
/// newest at the bottom), a wheel notch up over it shows the line before the first shown, and a new line while
/// looking back keeps the view; back at the end it follows again.
#[test]
fn the_lobby_chat_follows_the_newest_line_and_the_wheel_looks_back() {
    use dc_client::hud::DrawItem;
    let Some(mut m) = menu() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    m.data.chat = (0..40).map(|i| format!("chat line {i:02}")).collect();
    m.open(Go::Lobby);
    let shown = |m: &mut Menu| -> Vec<(String, f32)> {
        let mut v: Vec<(String, f32)> = m
            .items()
            .into_iter()
            .filter_map(|i| match i {
                DrawItem::Text { text, pos, .. } if text.starts_with("chat line") => {
                    Some((text, pos[1]))
                }
                DrawItem::Label { text, pos, .. } if text.starts_with("chat line") => {
                    Some((text, pos[1]))
                }
                _ => None,
            })
            .collect();
        v.sort_by(|a, b| a.1.total_cmp(&b.1));
        v
    };
    let v = shown(&mut m);
    // the bar at its end (range - page): the rows whose top is under the value, down to the list's bottom
    assert_eq!(
        v.last().unwrap().0,
        "chat line 39",
        "the newest at the bottom: {v:?}"
    );
    assert_eq!(v.len(), 6, "six lines as the reference stack: {v:?}");
    assert_eq!(v[0].0, "chat line 34");
    shot(&mut m, "lobby_chat_end");
    let list = m
        .sheet()
        .part(
            dc_client::menu::lobby::CHAT_PANEL,
            dc_client::menu::lobby::CHAT_LIST,
            WidgetKind::List,
        )
        .unwrap()
        .clone();
    let over = [(list.pos[0] + 40) as f32, (list.pos[1] + 20) as f32];
    m.input(MenuInput::Wheel(over, 1));
    let v = shown(&mut m);
    assert_eq!(v[0].0, "chat line 33", "one line back: {v:?}");
    assert_eq!(v.last().unwrap().0, "chat line 38");
    shot(&mut m, "lobby_chat_back1");
    // a new line while looking back: the view stays
    m.data.chat.push("chat line 40".into());
    let v = shown(&mut m);
    assert_eq!(v[0].0, "chat line 33", "the view stays");
    // back at the end it follows the newest again
    m.input(MenuInput::Wheel(over, -5));
    m.data.chat.push("chat line 41".into());
    let v = shown(&mut m);
    assert_eq!(v.last().unwrap().0, "chat line 41");
}
