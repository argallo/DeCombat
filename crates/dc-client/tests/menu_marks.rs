//! round 10 (CLI-911): the clan mark is the texture's top-left 51 x 51 texels (the art; the 64 x 64 texture is
//! padded): over the frame's rectangle on the clan page, 21 x 21 (scale 21 / 51) at the frame + (2, 2) on the lobby
//! panel. The whole padded texture over the frame drew a smaller mark pushed up and left.
//!
//! ROM-gated (skipped without `CA_ROM`).
// EVIDENCE: cshell+0x343cc0 / 0x343d50, DISASM cshell+0x121663..0x121693; OBSERVED the reference client (pairs/item3_marks.png)

use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::{Go, Menu, MenuOptions};

fn marks(m: &mut Menu) -> Vec<([f32; 2], [f32; 4])> {
    m.items()
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { tex, src, dst, .. } if tex.starts_with("ClanMark") => {
                Some((*src, *dst))
            }
            _ => None,
        })
        .collect()
}

#[test]
fn rom_clan_marks_show_the_51_texel_art_not_the_padded_texture() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("skipped: no CA_ROM");
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
    m.data.char_extras.clan_id = 7;
    m.data.char_extras.clan_mark = 1;
    m.data.char_extras.clan_mark_ground = 1;
    m.open(Go::Lobby);
    let lobby = marks(&mut m);
    assert_eq!(lobby.len(), 2, "background and emblem");
    for (src, dst) in &lobby {
        assert_eq!(*src, [51.0, 51.0]);
        // the frame is at (602, 627): the control sits 2 px in and is 21 x 21
        assert_eq!(*dst, [604.0, 629.0, 21.0, 21.0]);
    }
    m.open(Go::Clan);
    let clan = marks(&mut m);
    assert_eq!(clan.len(), 2);
    for (src, dst) in &clan {
        assert_eq!(*src, [51.0, 51.0]);
        assert_eq!(*dst, [33.0, 133.0, 51.0, 51.0]);
    }
}
