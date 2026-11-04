//! Community maps in the menu (MAP-32), headless and without a ROM: the map rows of the waiting room's map list come
//! after the ROM's rows (none here, so from index 0) in the join report's order with the record's label; the
//! settings panel's label, description and picture and the loading page come from the record; an index past the
//! rows keeps today's fallback; a vanilla link (no join report) clears the rows when the room list opens.
//! Nothing of the ROM is in this file.

use dc_client::loading::LoadingPage;
use dc_client::menu::link::{LinkEvent, LinkOptions};
use dc_client::menu::room::Room;
use dc_client::menu::{Go, Menu, MenuOptions};
use dc_client::testkit::synth_font;
use dc_mod_lua::MapRecord;

fn menu() -> Menu {
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-community-maps"),
        rom: None,
        font: Some((synth_font(), 0)),
        width: 1024,
        height: 768,
    });
    m.detach_link();
    m
}

fn record(mod_id: &str, id: &str, name: &str) -> MapRecord {
    MapRecord {
        mod_id: mod_id.into(),
        patched_by: Vec::new(),
        id: id.into(),
        name: name.into(),
        description: Some(format!("{name} described")),
        world: format!("@mod:{mod_id}/assets/maps/{id}.dat"),
        modes: vec!["TDM".into(), "FFA".into()],
        goals: Vec::new(),
        round_seconds: None,
        picture: Some(format!("@mod:{mod_id}/assets/pictures/{id}.dtx")),
        loading: Some(format!("@mod:{mod_id}/assets/pictures/{id}_load.dtx")),
        radar: None,
        players: None,
    }
}

#[test]
fn community_map_rows_follow_the_rom_rows_with_the_records_labels() {
    let mut m = menu();
    m.open(Go::Room);
    // the join's records, in mod-set load order then key order (the order the report carries); a lobby that
    // publishes no base: the ROM row count (0 here) stands in
    m.set_community_maps(
        None,
        vec![
            record("valley_pack", "alpha_valley", "Alpha Valley"),
            record("valley_pack", "beta_yard", "Beta Yard"),
            record("zeta_mod", "crater", "Crater"),
        ],
    );
    // the lobby's 0x311 pairs: the three community indices, the first two for mode 0 only
    m.feed_event(LinkEvent::RoomSettings(vec![(0, 0x1), (1, 0x1), (2, 0x2)]));
    m.update();
    let rom_rows = m.cat.rom_map_count() as i32;
    assert_eq!(rom_rows, 0, "no ROM: the community rows start at 0");
    assert_eq!(m.data.room.mode, 0);
    let rows = Room::list_rows(&m.cat, &m.data, "map");
    assert_eq!(
        rows,
        vec![
            ("Alpha Valley".to_string(), 0),
            ("Beta Yard".to_string(), 1)
        ]
    );
    // the settings panel's texts and picture come from the record, whatever the mode
    assert_eq!(m.cat.map_label(2), "Crater");
    assert_eq!(m.cat.map_description(2, 0), "Crater described");
    assert_eq!(m.cat.map_description(2, 3), "Crater described");
    assert_eq!(
        m.cat.map_picture(1).as_deref(),
        Some("@mod:valley_pack/assets/pictures/beta_yard.dtx")
    );
    assert_eq!(
        m.cat.community_map(1).map(|r| r.id.as_str()),
        Some("beta_yard")
    );
    // past the rows: today's fallback
    assert_eq!(m.cat.map_label(3), "Map 3");
    assert_eq!(m.cat.map_picture(3), None);
    assert_eq!(m.cat.map_description(3, 0), "");
    assert!(m.cat.community_map(3).is_none());
    assert!(m.cat.community_map(-1).is_none());
    // the loading page of a community map is the record's one picture, no tiles
    let page = LoadingPage::from_catalog(&m.cat, 0, 0, 2);
    assert_eq!(
        page.whole.as_deref(),
        Some("@mod:valley_pack/assets/pictures/alpha_valley_load.dtx")
    );
    assert_eq!((page.top, page.bottom), (None, None));
    let bare = LoadingPage::from_catalog(&m.cat, 7, 0, 1);
    assert_eq!(bare.whole, None);
}

#[test]
fn the_room_list_opening_on_a_vanilla_link_clears_the_rows() {
    let mut m = menu();
    m.set_community_maps(
        None,
        vec![record("valley_pack", "alpha_valley", "Alpha Valley")],
    );
    assert_eq!(m.cat.map_label(0), "Alpha Valley");
    // the lobby screen opens on the join report (a detached link: vanilla, no maps, nothing installed)
    m.open(Go::Lobby);
    assert_eq!(m.cat.community_maps().len(), 0);
    assert_eq!(m.cat.map_label(0), "Map 0");
    assert!(dc_client::modassets::active().is_none());
}

/// With a ROM: the community rows come after the ROM's map rows and the ROM rows keep their labels.
#[test]
fn rom_community_rows_come_after_the_rom_map_rows() {
    let Ok(rom) = dc_rom::resolve_rom() else {
        println!("CA_ROM not set: skipped");
        return;
    };
    let mut m = Menu::new(MenuOptions {
        auth: "127.0.0.1:1".parse().unwrap(),
        user: "tester".into(),
        link: LinkOptions::new("menu-community-maps-rom"),
        rom: Some(rom),
        font: None,
        width: 1024,
        height: 768,
    });
    m.detach_link();
    let count = m.cat.rom_map_count();
    assert!(count > 0, "the ROM room table has map rows");
    let first_rom = m.cat.map_label(0);
    m.set_community_maps(
        None,
        vec![record("valley_pack", "alpha_valley", "Alpha Valley")],
    );
    assert_eq!(m.cat.map_label(0), first_rom, "ROM rows untouched");
    assert_eq!(m.cat.first_community_index(), count);
    assert_eq!(m.cat.map_label(count as i32), "Alpha Valley");
    assert_eq!(
        m.cat.map_label(count as i32 + 1),
        format!("Map {}", count + 1)
    );
    assert!(m.cat.community_map(count as i32 - 1).is_none());
    // the lobby's published base wins over this ROM's row count (a lobby without a ROM says 12)
    m.set_community_maps(
        Some(12),
        vec![record("valley_pack", "alpha_valley", "Alpha Valley")],
    );
    assert_eq!(m.cat.first_community_index(), 12);
    assert_eq!(m.cat.map_label(12), "Alpha Valley");
}

/// C6 / MAP-32: the lobby's `community_map_base` (its server list) places the community rows, not this client's
/// ROM: a lobby without a ROM announces its map at 12 and the client without a ROM shows it there, while the
/// indices below are the lobby's (synthetic) ROM rows.
#[test]
fn the_lobbys_published_base_places_the_community_rows() {
    let mut m = menu();
    m.open(Go::Room);
    m.set_community_maps(
        Some(12),
        vec![
            record("valley_pack", "alpha_valley", "Alpha Valley"),
            record("zeta_mod", "crater", "Crater"),
        ],
    );
    assert_eq!(m.cat.rom_map_count(), 0, "no ROM rows in this client");
    assert_eq!(m.cat.first_community_index(), 12);
    assert_eq!(
        m.cat.community_map(12).map(|r| r.id.as_str()),
        Some("alpha_valley")
    );
    assert_eq!(
        m.cat.community_map(13).map(|r| r.id.as_str()),
        Some("crater")
    );
    assert!(m.cat.community_map(11).is_none());
    assert!(m.cat.community_map(0).is_none());
    assert!(m.cat.community_map(14).is_none());
    assert_eq!(m.cat.map_label(12), "Alpha Valley");
    assert_eq!(
        m.cat.map_label(0),
        "Map 0",
        "the lobby's ROM row, unnamed here"
    );
    assert_eq!(
        m.cat.map_picture(13).as_deref(),
        Some("@mod:zeta_mod/assets/pictures/crater.dtx")
    );
    // the 0x311 pairs carry the lobby's indices; the room list labels them from the records
    m.feed_event(LinkEvent::RoomSettings(vec![(12, 0x1), (13, 0x1)]));
    m.update();
    let rows = Room::list_rows(&m.cat, &m.data, "map");
    assert_eq!(
        rows,
        vec![("Alpha Valley".to_string(), 12), ("Crater".to_string(), 13)]
    );
    let page = LoadingPage::from_catalog(&m.cat, 12, 0, 1);
    assert_eq!(
        page.whole.as_deref(),
        Some("@mod:valley_pack/assets/pictures/alpha_valley_load.dtx")
    );
    assert_eq!(LoadingPage::from_catalog(&m.cat, 0, 0, 1).whole, None);
}
