//! The lobby user list: every row's name sits vertically centred in its bar, for the own row (gold), the row under
//! the pointer (gold) and a plain row, at 4:3 and wide screen sizes. ROM-gated.
//!
//! The 2009 row: the bar at the row plus UserBarPos, the name control at the row plus UserNamePos, left aligned and
//! vertically centred about that point (flags 0x41), so the name's centre is UserNamePos.y - UserBarPos.y below the
//! bar's top (cshell+0x11fa00, cshell+0x120d90). OBSERVED: lobbyfx4/real/R10_lobby.png (1024 x 768: the bar at
//! y 110..141, the name's capitals 122..129).

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::UserRow;
use dc_client::menu::lobby::RowStyle;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};

const BAR_TEX: &str = "User_List_Panel";

fn lobby(rom: std::path::PathBuf, w: u32, h: u32) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: dc_client::menu::link::LinkOptions::new("user-rows"),
        rom: Some(rom),
        font: None,
        width: w,
        height: h,
    });
    m.detach_link();
    m.data.profile.my_id = 1;
    m.data.users = (1..=3)
        .map(|i| UserRow {
            account: i,
            id: i,
            name: format!("Realone{i}"),
            exp: 0,
            ..Default::default()
        })
        .collect();
    m.open(Go::Lobby);
    for _ in 0..10 {
        m.update();
    }
    m
}

/// (bar rectangle, name position, name anchor, name colour) per user row, in virtual units.
fn rows(items: &[DrawItem]) -> Vec<([f32; 4], [f32; 2], [f32; 2], [u8; 4])> {
    let mut out = Vec::new();
    let mut bar = None;
    for it in items {
        match it {
            DrawItem::Image { tex, dst, .. } if tex.contains(BAR_TEX) => bar = Some(*dst),
            DrawItem::Label {
                text,
                pos,
                anchor,
                style,
            } if text.starts_with("Realone") => {
                out.push((bar.take().expect("a bar before the name"), *pos, *anchor, style.colour));
            }
            _ => {}
        }
    }
    out
}

#[test]
fn rom_user_row_names_sit_centred_in_their_bars() {
    let Some(rom) = dc_rom::test_rom() else {
        println!("skipped: CA_ROM unset");
        return;
    };
    for (w, h) in [(1024u32, 768u32), (1920, 1080), (2560, 1440)] {
        let mut m = lobby(rom.clone(), w, h);
        let st = RowStyle::load(&m.cat);
        let first = rows(&m.items());
        assert_eq!(first.len(), 3, "{w}x{h}: three rows with their bars");
        // the pointer on the third row: its name turns gold like the own (first) row
        let b = first[2].0;
        m.input(MenuInput::Move([b[0] + 60.0, b[1] + b[3] / 2.0]));
        m.update();
        let items = m.items();
        let r = rows(&items);
        let gold = r[0].3;
        assert_eq!(r[2].3, gold, "{w}x{h}: the pointed row is gold");
        assert_ne!(r[1].3, gold, "{w}x{h}: a plain row");
        // structure: every name is centred (anchor y 0.5) the 2009 distance below its bar's top
        let want = (st.user_name_at[1] - st.user_bar_at[1]) as f32;
        for (i, (bar, pos, anchor, _)) in r.iter().enumerate() {
            assert_eq!(anchor[1], 0.5, "{w}x{h} row {i}: vertically centred");
            assert_eq!(pos[1] - bar[1], want, "{w}x{h} row {i}: name centre below the bar's top");
        }
        // pixels: the name's ink is centred in its bar within a pixel and a half of the screen scale
        let img = m.cat.compose(&items);
        let (sx, sy) = (w as f32 / 1024.0, h as f32 / 768.0);
        for (i, (bar, pos, _, colour)) in r.iter().enumerate() {
            let (top, bottom) = (bar[1] * sy, (bar[1] + bar[3]) * sy);
            let x0 = (pos[0] * sx) as u32;
            let x1 = ((bar[0] + bar[2]) * sx) as u32 - 1;
            let ink: Vec<u32> = (top.ceil() as u32..bottom.floor() as u32)
                .filter(|&y| {
                    (x0..x1).any(|x| {
                        let p = img.pixel(x, y);
                        (0..3).all(|c| p[c].abs_diff(colour[c]) < 40)
                    })
                })
                .collect();
            assert!(!ink.is_empty(), "{w}x{h} row {i}: the name is drawn inside its bar");
            let (a, z) = (ink[0] as f32, *ink.last().unwrap() as f32 + 1.0);
            let off = (a + z) / 2.0 - (top + bottom) / 2.0;
            println!("{w}x{h} row {i}: ink {a}..{z} in bar {top:.1}..{bottom:.1}, offset {off:.2}");
            assert!(off.abs() <= 1.5 * sy.max(1.0), "{w}x{h} row {i}: name {off:.2} px off its bar's centre");
        }
    }
}
