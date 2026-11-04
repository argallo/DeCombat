//! lobby-leftovers11: the waiting room's own-team look as 2009 sets it (cshell+0x15d9d0, cshell+0x1657e0): on team
//! Alpha the team button is the one towards Bravo and the Alpha label is on; the own name is drawn 0xff61ffff; the map
//! description lines are 16 px apart. ROM-gated (skipped without `CA_ROM`).

use dc_botclient::lobby::RoomMember;
use dc_client::hud::items::DrawItem;
use dc_client::menu::link::LinkOptions;
use dc_client::menu::room::{
    MAP_DESCRIPTION, OWN_NAME_COLOUR, SETTINGS_WINDOW, TEAM_ALPHA_BUTTON, TEAM_ALPHA_LABEL,
    TEAM_BRAVO_BUTTON, TEAM_BRAVO_LABEL, TEAM_WINDOW,
};
use dc_client::menu::roomtext::DESCRIPTION_PITCH;
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_ui_data::WidgetKind;

fn menu() -> Option<Menu> {
    let rom = dc_rom::resolve_rom().ok()?;
    Some(Menu::new(MenuOptions {
        auth: "127.0.0.1:9".parse().unwrap(),
        user: "t".into(),
        link: LinkOptions::new("t"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    }))
}

#[test]
fn rom_alpha_member_sees_the_bravo_button_lit_label_and_cyan_name() {
    let Some(mut m) = menu() else {
        eprintln!("skipped: no CA_ROM");
        return;
    };
    m.data.profile.my_id = 7;
    m.data.room.host_id = 7;
    m.data.room.map = 2;
    m.data.room.mode = 8;
    m.data.room.max_players = 16;
    m.data.members.insert(
        7,
        RoomMember {
            char_no: 7,
            name: "Me".into(),
            team: 1,
            state: 0,
            exp: 0,
        },
    );
    m.open(Go::Room);
    let items = m.items();
    let shown = |m: &Menu, id, kind| m.sheet().part(TEAM_WINDOW, id, kind).map(|p| p.shown);
    assert_eq!(
        shown(&m, TEAM_ALPHA_BUTTON, WidgetKind::Button),
        Some(false)
    );
    assert_eq!(shown(&m, TEAM_BRAVO_BUTTON, WidgetKind::Button), Some(true));
    let checked = |m: &Menu, id| {
        m.sheet()
            .part(TEAM_WINDOW, id, WidgetKind::CheckButton)
            .map(|p| p.checked)
    };
    assert_eq!(checked(&m, TEAM_ALPHA_LABEL), Some(true));
    assert_eq!(checked(&m, TEAM_BRAVO_LABEL), Some(false));
    assert!(items
        .iter()
        .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex.contains("Team_change_Bravo_"))));
    assert!(items
        .iter()
        .any(|i| matches!(i, DrawItem::Image { tex, .. } if tex.contains("Team_Label_ON"))));
    let name = items.iter().find_map(|i| match i {
        DrawItem::Label { text, style, .. } if text == "Me" => Some(style.colour),
        _ => None,
    });
    let want = OWN_NAME_COLOUR.to_be_bytes();
    assert_eq!(name, Some([want[1], want[2], want[3], want[0]]));
    // the description's lines 16 px apart
    let at = m
        .sheet()
        .part(SETTINGS_WINDOW, MAP_DESCRIPTION, WidgetKind::Text)
        .unwrap()
        .pos;
    let ys: Vec<f32> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Label { pos, .. }
                if pos[0] == at[0] as f32
                    && pos[1] >= at[1] as f32
                    && pos[1] < at[1] as f32 + 120.0 =>
            {
                Some(pos[1])
            }
            _ => None,
        })
        .collect();
    assert!(ys.len() >= 3, "{ys:?}");
    assert!(
        ys.windows(2)
            .all(|w| w[1] - w[0] == DESCRIPTION_PITCH as f32),
        "{ys:?}"
    );
}
