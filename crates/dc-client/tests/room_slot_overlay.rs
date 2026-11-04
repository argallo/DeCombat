//! The waiting room's own-row bar covers the list's slot art: the bar is placed at the row plus the section's bar
//! position, five pixels above the row like the master badge, so its top meets the slot's top and its bottom the
//! slot's lower edge. Checked for the own row as the host, the own row below the host, and the own row on Bravo,
//! at 1024 x 768, 1920 x 1080, 2560 x 1440 and a 2x (Retina-like) 2048 x 1536. ROM-gated.
//!
//! DISASM: cshell+0x15c660 (each row control at the row plus its section-9 position, the bar's included).
//! OBSERVED: lobbyfx4/real/R20_room.png (1024 x 768: the host's bar on rows 227..254, the slot art 226..254, the
//! separator below it from 255).

use dc_botclient::lobby::RoomMember;
use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::room::{MemberStyle, TEAM_A_ROWS, TEAM_B_ROWS, TEAM_WINDOW};
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_ui_data::WidgetKind;

const HOST: u32 = 7;
/// (id, name, team): the host and a second member on Alpha, one member on Bravo.
const MEMBERS: [(u32, &str, i32); 3] = [(7, "mactester", 1), (8, "other", 1), (9, "bravo", 2)];
const NOBODY: u32 = 999;

fn room(rom: std::path::PathBuf, w: u32, h: u32) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("room-slot-overlay"),
        rom: Some(rom),
        font: None,
        width: w,
        height: h,
    });
    m.detach_link();
    m.data.profile.my_id = HOST;
    m.data.room.host_id = HOST;
    m.data.room.map = 2;
    m.data.room.mode = 8;
    m.data.room.max_players = 16;
    for (id, name, team) in MEMBERS {
        m.data.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: name.into(),
                team,
                state: 0,
                exp: 0,
            },
        );
    }
    m.open(Go::Room);
    for _ in 0..5 {
        m.update();
    }
    m
}

/// Mean of the colour sums over `x0..x1` on screen row `y`.
fn level(img: &dc_client::overlay::text::RgbaImage, x0: u32, x1: u32, y: u32) -> f32 {
    let s: u32 = (x0..x1)
        .map(|x| {
            let p = img.pixel(x, y);
            p[0] as u32 + p[1] as u32 + p[2] as u32
        })
        .sum();
    s as f32 / (x1 - x0) as f32
}

#[test]
fn rom_own_row_bar_covers_its_slot_art() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    // (own id, its list, its row in that list)
    let cases = [(7u32, TEAM_A_ROWS, 0i32), (8, TEAM_A_ROWS, 1), (9, TEAM_B_ROWS, 0)];
    for (w, h) in [(1024u32, 768u32), (1920, 1080), (2560, 1440), (2048, 1536)] {
        let mut m = room(rom.clone(), w, h);
        let st = MemberStyle::load(&m.cat);
        let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
        for (own, list, row) in cases {
            let tag = format!("{w}x{h} own {own}");
            // without an own row (nobody of the room is this client): the bare slot art
            m.data.profile.my_id = NOBODY;
            let bare_items = m.items();
            assert!(
                !bare_items
                    .iter()
                    .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex.contains("Waiting_Over_Bar"))),
                "{tag}: no bar without an own row (host and other rows draw none)"
            );
            let bare = m.cat.compose(&bare_items);
            m.data.profile.my_id = own;
            let items = m.items();
            let bars: Vec<[f32; 4]> = items
                .iter()
                .filter_map(|i| match i {
                    DrawItem::Image { tex, dst, .. } if tex.contains("Waiting_Over_Bar") => Some(*dst),
                    _ => None,
                })
                .collect();
            assert_eq!(bars.len(), 1, "{tag}: one bar, on the own row only");
            let bar = bars[0];
            // structure: the bar at the row plus the section's bar position
            let l = m.sheet().part(TEAM_WINDOW, list, WidgetKind::List).expect("member list");
            let pitch = st.bar[1] + 4;
            let want = [
                (l.pos[0] + st.bar_at[0]) as f32,
                (l.pos[1] + row * pitch + st.bar_at[1]) as f32,
                st.bar[0] as f32,
                st.bar[1] as f32,
            ];
            assert_eq!(bar, want, "{tag}: bar rectangle");
            let img = m.cat.compose(&items);
            // a span between the clan and status columns, clear of text
            let x0 = ((bar[0] + 200.0) * sx) as u32;
            let x1 = ((bar[0] + 260.0) * sx) as u32;
            // the bar's screen rows: where the frame differs from the bare one
            let band = |y: u32| {
                (x0..x1).any(|x| {
                    let (a, b) = (img.pixel(x, y), bare.pixel(x, y));
                    (0..3).any(|c| a[c].abs_diff(b[c]) > 3)
                })
            };
            let lo = ((bar[1] - 12.0) * sy) as u32;
            let hi = ((bar[1] + bar[3] + 12.0) * sy) as u32;
            let drawn: Vec<u32> = (lo..hi).filter(|&y| band(y)).collect();
            assert!(!drawn.is_empty(), "{tag}: the bar is visible");
            let (top, bottom) = (drawn[0], *drawn.last().unwrap() + 1);
            assert_eq!(
                drawn.len() as u32,
                bottom - top,
                "{tag}: one solid band (no second row lit)"
            );
            // the slot art's edges in the bare frame: the strongest brightening near the bar's top and the strongest
            // darkening near its bottom, each a real edge (the slot art against the header or a separator), not noise
            let step = |y: u32| level(&bare, x0, x1, y) - level(&bare, x0, x1, y - 1);
            let reach = (4.0 * sy).ceil() as u32;
            let edge = |at: u32, sign: f32| {
                let y = (at - reach..=at + reach)
                    .max_by(|&a, &b| (sign * step(a)).total_cmp(&(sign * step(b))))
                    .unwrap();
                assert!(
                    sign * step(y) > 15.0,
                    "{tag}: no slot edge within {reach} rows of {at} (best {y}: {:.1})",
                    step(y)
                );
                y
            };
            let slot_top = edge(top, 1.0);
            let slot_bottom = edge(bottom, -1.0);
            println!("{tag}: bar rows {top}..{bottom}, slot art {slot_top}..{slot_bottom}");
            // 2009 draws its bar one virtual pixel under the slot's top edge and ends it on the slot's lower edge
            let tol = (1.0 * sy).ceil() as i64 + 1;
            assert!(
                (top as i64 - slot_top as i64).abs() <= tol,
                "{tag}: bar top {top} vs slot top {slot_top}"
            );
            assert!(
                (bottom as i64 - slot_bottom as i64).abs() <= tol,
                "{tag}: bar bottom {bottom} vs slot bottom {slot_bottom}"
            );
        }
    }
}
