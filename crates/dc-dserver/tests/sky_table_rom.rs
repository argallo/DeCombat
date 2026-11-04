//! ROM-gated (skipped without CA_ROM): the sky table of the worlds that carry two sky models in one slot. The user's
//! "Two Towers: the sky is red instead of daytime": TWOTOWERS holds a day sky box and, later in the file, the red
//! sky box of its quarantine variant; every start trigger of its other modes switches the second one off. With the
//! room's start triggers the table names the day box and takes its block (DSV-520); the quarantine room's triggers
//! are reported. Names never appear in the output: slot indices into the world's sky models only.
// DISASM: engine+0x8b7d0, engine+0x96ef0 (see `dc_dserver::worldobj::scene_brush`)

use dc_dserver::world::{load_from_rom, BrushClass, NameForm};
use dc_dserver::worldobj::{plan_scene, sky_table, WorldObject, WorldObjects, ZoneClass};

/// The room's `trig` setting for this world in a room of the mode with this key, as our lobby writes it.
fn trig_for(rom: &std::path::Path, world: &str, key: &str) -> Option<String> {
    let vfs = dc_rez::RezVfs::new(rom);
    let gr = dc_bute::GameRoomData::from_vfs(&vfs).ok()?;
    let unq = |s: &str| s.trim().trim_matches('"').to_string();
    let mode = gr.modes(false).iter().position(|m| {
        m.get("Key")
            .is_some_and(|k| unq(k).eq_ignore_ascii_case(key))
    })?;
    let base = |p: &str| {
        let n = p.replace('/', "\\").to_ascii_uppercase();
        let b = n.rsplit('\\').next().unwrap_or(&n).to_string();
        b.strip_suffix(".DAT").map(str::to_string).unwrap_or(b)
    };
    let want = base(world);
    for (i, m) in gr.maps().iter().enumerate() {
        if !m.contains_key(&format!("GMS_{key}")) {
            continue;
        }
        let (_md, doc) = dc_bute::gms_file_with_vfs(&vfs, &gr, i, mode, false);
        if doc
            .get_str("MapInfo", "Map")
            .is_some_and(|w| base(&unq(w)) == want)
        {
            let cfg = dc_bute::RoomConfig {
                map: i,
                mode,
                ..dc_bute::RoomConfig::default()
            };
            let line =
                dc_bute::room_line_with_vfs(&vfs, &gr, &cfg, &dc_bute::RoomOptions::default());
            return match dc_dserver::pipe::parse_control(&line) {
                dc_dserver::pipe::Control::Room(r) => r.get("trig").map(str::to_string),
                _ => None,
            };
        }
    }
    None
}

/// The index (among the world's sky models, file order) of the model the planned sky table names in slot 0.
fn slot0(
    w: &dc_dserver::world::WorldInfo,
    tables: &dc_dserver::rom_tables::RomTables,
    trig: Option<&str>,
) -> Option<usize> {
    let mut next = 1000u16..;
    let objs = plan_scene(
        w,
        tables,
        WorldObjects::World,
        ZoneClass::Room,
        1,
        trig,
        || next.next().unwrap(),
    );
    let (_, ids) = sky_table(&objs)?;
    let name = objs.iter().find_map(|o| match o {
        WorldObject::Brush { obj, brush } if *obj == ids[0] => {
            Some(brush.name.to_ascii_lowercase())
        }
        _ => None,
    })?;
    w.brushes
        .iter()
        .filter(|b| b.class == BrushClass::Sky)
        .position(|b| b.name.to_ascii_lowercase() == name)
}

#[test]
fn rom_two_towers_rooms_show_the_day_sky() {
    let rom = dc_rez::dc_rom::rom_or_skip!();
    let (tables, _) = dc_dserver::rom_tables::RomTables::load(&rom);
    let w = load_from_rom(&rom, "TWOTOWERS", NameForm::Listed, None).expect("the world");
    let skies = w
        .brushes
        .iter()
        .filter(|b| b.class == BrushClass::Sky)
        .count();
    assert_eq!(skies, 2, "a day sky and the quarantine sky");
    assert_eq!(
        slot0(&w, &tables, None),
        Some(1),
        "without scripts the later model takes the slot"
    );
    let mut rooms = 0;
    for key in ["TDM", "CTF", "BNT", "TSV", "F", "CT", "SPY", "QRT"] {
        let Some(trig) = trig_for(&rom, "TWOTOWERS", key) else {
            continue;
        };
        let got = slot0(&w, &tables, Some(&trig));
        println!("{key}: slot 0 holds sky model {got:?}");
        if key != "QRT" {
            assert_eq!(got, Some(0), "{key}: the day sky");
            rooms += 1;
        }
    }
    assert!(rooms > 0, "at least one mode room of the world");
}
