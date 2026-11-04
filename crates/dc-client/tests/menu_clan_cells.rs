//! CLI-1022: the create page's emblem / background cells draw the 51 x 51 texel square of the 64 x 64 mark textures
//! (CLI-911) over the cell's inside, so the art is centred in its selection box. ROM-gated.

use dc_client::hud::items::DrawItem;
use dc_client::menu::clan::{CMD_CREATE, MARK_TEXELS};
use dc_client::menu::clanpage::{CELL, CELL_STEP, GRID_AT, GRID_DY};
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuInput, MenuOptions};

#[test]
fn rom_create_page_cells_draw_the_mark_square_centred() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
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
    m.open(Go::Clan);
    for _ in 0..300 {
        m.update();
        if m.screen_name() == "clan" && m.fading().is_none() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    m.input(MenuInput::Command(CMD_CREATE));
    let items = m.items_at(0.5);
    let src = MARK_TEXELS.map(|v| v as f32);
    for g in 0..2 {
        let cell = [GRID_AT[0], GRID_AT[1] + g * GRID_DY, CELL, CELL];
        let inside: Vec<_> = items
            .iter()
            .filter_map(|i| match i {
                DrawItem::Image { src: s, dst, .. }
                    if dst[0] >= cell[0] as f32
                        && dst[1] >= cell[1] as f32
                        && dst[0] + dst[2] <= (cell[0] + cell[2]) as f32
                        && dst[1] + dst[3] <= (cell[1] + cell[3]) as f32 =>
                {
                    Some((*s, *dst))
                }
                _ => None,
            })
            .collect();
        assert!(
            !inside.is_empty(),
            "grid {g}: mark layers in the first cell"
        );
        for (s, dst) in inside {
            assert_eq!(s, src, "grid {g}: the 51 x 51 texel square");
            // centred: the same margin on both sides
            let (l, r) = (
                dst[0] - cell[0] as f32,
                (cell[0] + cell[2]) as f32 - dst[0] - dst[2],
            );
            let (t, b) = (
                dst[1] - cell[1] as f32,
                (cell[1] + cell[3]) as f32 - dst[1] - dst[3],
            );
            assert!(
                (l - r).abs() < 0.6 && (t - b).abs() < 0.6,
                "grid {g}: centred {dst:?}"
            );
        }
    }
    let _ = CELL_STEP;
}
