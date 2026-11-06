//! CLI-1895 (owner report 2026-11-05 "Clan name does not appear in the lobby of a game"): the waiting room's member
//! rows draw a member's clan name (in the clan colour, else the row's clan colour 0xff9898ff) and clan mark (both
//! ids set) at the row plus the layout's clan name / clan mark places; a member without a clan shows neither.
//!
//! ROM-gated (skipped without `CA_ROM`), no lobby. With `DC_ROOM_CLAN_PNG` set to a folder the screen is written to
//! `room_clan_rows.png` there.
// EVIDENCE: cshell+0x166310 (the member refresh: clan name, clan colour and mark pair from the record onto the row),
// cshell+0x1657e0 (the clan text's own colour 0xff9898ff without a clan colour)
// DISASM: cshell+0x15c83d / 0x15c894 (the clan text and the mark placed at the row plus their layout keys)

use dc_botclient::lobby::RoomMember;
use dc_client::hud::items::DrawItem;
use dc_client::menu::link::{LinkOptions, MemberClan};
use dc_client::menu::room::CLAN_NAME_COLOUR;
use dc_client::menu::{Go, Menu, MenuOptions};

fn rgba(c: u32) -> [u8; 4] {
    [(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]
}

fn room_menu(rom: std::path::PathBuf) -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("room-clan-test"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let d = &mut m.data;
    d.profile.my_id = 1;
    d.room.host_id = 1;
    d.room.mode = 0;
    d.room.max_players = 8;
    for (id, team) in [(1u32, 1), (2, 2), (3, 1)] {
        d.members.insert(
            id,
            RoomMember {
                char_no: id as i32,
                name: format!("member{id}"),
                team,
                state: 0,
                exp: 0,
            },
        );
    }
    m
}

fn label<'a>(items: &'a [DrawItem], want: &str) -> Option<(&'a [f32; 2], [u8; 4])> {
    items.iter().find_map(|i| match i {
        DrawItem::Label {
            text, pos, style, ..
        } if text == want => Some((pos, style.colour)),
        _ => None,
    })
}

#[test]
fn rom_member_rows_show_the_clan_name_and_mark() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let word = dc_client::menu::charinfo::table_colour(Some(&rom), 2)
        .expect("the ROM colour table has a second entry");
    let mut m = room_menu(rom);
    m.data.member_clan.insert(
        2,
        MemberClan {
            id: 9,
            name: "Wolves".into(),
            mark: 1,
            ground: 1,
        },
    );
    m.data.member_clan.insert(
        3,
        MemberClan {
            id: 4,
            name: "Foxes".into(),
            mark: 0,
            ground: 0,
        },
    );
    m.data.member_clan.insert(1, MemberClan::default());
    // member 3's clan has a clan colour item (record +0xb0), member 2's none
    m.data.member_colour.insert(3, (0, 2));
    m.open(Go::Room);
    let items = m.items_at(0.0);
    let (name2, _) = label(&items, "member2").expect("member2's row");
    let (clan2, c2) = label(&items, "Wolves").expect("member2's clan name is drawn");
    assert_eq!(
        c2,
        rgba(CLAN_NAME_COLOUR),
        "no clan colour: the row's clan colour"
    );
    assert_eq!(clan2[1], name2[1], "on the name's line");
    assert!(
        clan2[0] > name2[0] + 60.0,
        "after the name: {clan2:?} vs {name2:?}"
    );
    let (_, c3) = label(&items, "Foxes").expect("member3's clan name is drawn");
    assert_eq!(c3, rgba(word), "the clan colour item's colour");
    assert!(
        label(&items, "member1").is_some(),
        "the clanless member's row is drawn"
    );
    // the mark of member 2 (both ids set) is drawn 21 x 21 on its row, left of the clan name; member 3 has none
    let marks: Vec<[f32; 4]> = items
        .iter()
        .filter_map(|i| match i {
            DrawItem::Image { dst, .. }
                if dst[2] == 21.0 && dst[3] == 21.0 && (dst[1] - name2[1]).abs() < 28.0 =>
            {
                Some(*dst)
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        marks.len(),
        2,
        "emblem and background of one mark: {marks:?}"
    );
    assert!(
        marks.iter().all(|d| d[0] < clan2[0] && d[0] > name2[0]),
        "{marks:?}"
    );
    if let Ok(dir) = std::env::var("DC_ROOM_CLAN_PNG") {
        let i = m.image();
        let f = dc_render::FrameImage {
            width: i.width,
            height: i.height,
            rgba: i.rgba,
        };
        f.save_png(&std::path::Path::new(&dir).join("room_clan_rows.png"))
            .expect("png written");
    }
}
