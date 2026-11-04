//! The server on a community map written in DeCombat's own vocabulary (no ROM content): the spawn sets per mode
//! list (MAP-21), the team texts (MAP-22), the world objects a room creates (bomb sites, flag zones, the capture
//! flags with the default look, MAP-23), the sky table of the `dc.sky_model`, the sounds and sprites, and the sim
//! side (ladder volume, collision set) of the same file.

use dc_dserver::game::{Game, GameTuning};
use dc_dserver::pipe::RoomSettings;
use dc_dserver::rom_tables::RomTables;
use dc_dserver::world::{synthetic_dc_world_bytes, world_from_bytes, WorldInfo, ZoneKind};
use dc_dserver::worldobj::{self, WorldObject, WorldObjects, ZoneClass, NO_MODEL_FILE};

fn world() -> WorldInfo {
    world_from_bytes(
        &synthetic_dc_world_bytes(),
        "@mod:test/assets/maps/synth_dc.dat",
        "dc map",
    )
    .unwrap()
}

/// A game in a room of the given mode class (no ROM tables: the team texts `"1"` / `"2"` need none); the room key
/// `ZZ` matches no ROM-style filter.
fn game_in(class: u8, world: &WorldInfo) -> Game {
    let mut g = Game::new(
        GameTuning {
            trace: false,
            ..GameTuning::default()
        },
        world,
        RomTables::default(),
    );
    let mut room = RoomSettings::default();
    room.pairs.insert("type".into(), class.to_string());
    room.pairs.insert("key".into(), "ZZ".into());
    g.set_room(&room);
    g
}

/// MAP-21: the spawn set per room mode on the synthetic map (points: t1_a, t1_b, t2_a, t2_b tagged `TDM,FFA`;
/// bomb_t1, bomb_t2 tagged `BOMB`; `any` with an empty filter; flag_t1 tagged `flag`).
#[test]
fn spawn_sets_follow_the_mode_lists() {
    let w = world();
    let set = |class: u8| game_in(class, &w).spawn_set();
    let t = true;
    let f = false;
    assert_eq!(set(1), vec![t, t, t, t, f, f, t, f], "TDM");
    assert_eq!(
        set(4),
        vec![t, t, t, t, f, f, t, f],
        "TDM_CLAN serves the TDM set"
    );
    assert_eq!(set(6), vec![t, t, t, t, f, f, t, f], "FFA");
    assert_eq!(set(2), vec![f, f, f, f, t, t, t, f], "BOMB");
    assert_eq!(set(3), vec![f, f, f, f, t, t, t, f], "BOMB_CLAN");
    assert_eq!(
        set(12),
        vec![f, f, f, f, t, t, t, f],
        "ELIM uses the BOMB set"
    );
    assert_eq!(set(13), vec![f, f, f, f, t, t, t, f], "ELIM_CLAN");
    assert_eq!(
        set(7),
        vec![f, f, f, f, f, f, t, t],
        "FLAG (lower-case tag)"
    );
    assert_eq!(set(8), vec![f, f, f, f, f, f, t, t], "FLAG_CLAN");
    assert_eq!(
        set(10),
        vec![f, f, f, f, f, f, t, f],
        "a class without a name: the empty filter only"
    );
    assert_eq!(game_in(1, &w).dc_mode_name(), Some("TDM"));
    assert_eq!(game_in(10, &w).dc_mode_name(), None);
    // the team pick: TDM team 1 draws from its two TDM points, never the bomb or flag points
    let mut g = game_in(1, &w);
    for _ in 0..8 {
        let (p, _) = g.choose_spawn(1);
        assert!(
            p[0] == -800.0 && (p[2] == -800.0 || p[2] == -700.0),
            "{p:?}"
        );
        let (p, _) = g.choose_spawn(2);
        assert!(p[0] == 800.0 && (p[2] == 800.0 || p[2] == 700.0), "{p:?}");
    }
    let mut g = game_in(2, &w);
    assert_eq!(g.choose_spawn(1).0, [-900.0, 40.0, 0.0]);
    assert_eq!(g.choose_spawn(2).0, [900.0, 40.0, 0.0]);
    // ELIM on a map without a BOMB set falls back to the TDM points (DSV-470 in DeCombat names)
    let mut w2 = world();
    for m in w2.spawn_modes.iter_mut() {
        if m.as_deref() == Some(b"BOMB") {
            *m = Some(b"FLAG".to_vec());
        }
    }
    assert_eq!(game_in(12, &w2).spawn_set(), vec![t, t, t, t, f, f, t, f]);
    // a ROM-style filter on the same map keeps the initials rule: the room key `ZZ` matches `Zed Zone` only
    let mut w3 = world();
    w3.spawn_modes[0] = Some(b"Zone of zebras".to_vec());
    assert!(!game_in(1, &w3).spawn_set()[0]);
    w3.spawn_modes[0] = Some(b"Zed Zone".to_vec());
    assert!(game_in(1, &w3).spawn_set()[0]);
}

/// MAP-22 / MAP-23: the room's world objects without ROM tables: zone teams from the texts `"1"` / `"2"` (a bomb
/// site without `defuse_team` is any team), both capture flags created, team 1's with no model file (the default
/// look) and no file-list entry, team 2's with its own model path; the sky table from the `dc.sky_model`; the
/// sound and the sprite.
#[test]
fn world_objects_of_a_community_map() {
    let w = world();
    let tables = RomTables::default();
    let mut next = 100u16;
    let mut id = || {
        next += 1;
        next
    };
    let objs = worldobj::plan(&w, &tables, WorldObjects::All, ZoneClass::Own, 7, &mut id);
    let zones: Vec<(ZoneKind, u8, i32)> = objs
        .iter()
        .filter_map(|o| match o {
            WorldObject::Zone { zone, team, .. } => Some((zone.kind, *team, zone.number)),
            _ => None,
        })
        .collect();
    assert_eq!(
        zones,
        vec![
            (ZoneKind::BombSite, worldobj::ZONE_TEAM_ANY, 0),
            (ZoneKind::BombSite, 2, 1),
            (ZoneKind::FlagZone, 1, 0),
            (ZoneKind::FlagZone, 2, 0),
        ]
    );
    let flags: Vec<(u32, &str, bool, Vec<String>)> = objs
        .iter()
        .filter_map(|o| match o {
            WorldObject::Flag {
                team, model, shown, ..
            } => Some((*team, model.as_str(), *shown, o.files())),
            _ => None,
        })
        .collect();
    assert_eq!(
        flags,
        vec![
            (1, "", true, vec![]),
            (
                2,
                "@mod:test/assets/flag.ltb",
                true,
                vec!["@mod:test/assets/flag.ltb".to_string()]
            ),
        ]
    );
    // the no-model flag's create carries the no-model file id; the file-id closure is never asked for ""
    let pbox = dc_dserver::objects::PosBox {
        min: w.extent_min,
        max: w.extent_max,
    };
    let flag = objs
        .iter()
        .find(|o| matches!(o, WorldObject::Flag { team: 1, .. }))
        .unwrap();
    let rec = flag.record(&pbox, &|p| {
        assert!(!p.is_empty(), "no lookup of an empty path");
        7
    });
    let with_model = objs
        .iter()
        .find(|o| matches!(o, WorldObject::Flag { team: 2, .. }))
        .unwrap()
        .record(&pbox, &|_| 7);
    assert_ne!(rec.as_bytes(), with_model.as_bytes());
    assert_eq!(NO_MODEL_FILE, 0xFFFF);
    // the sky table: slot 0 = the sky brush, the block from its position, dims and inner fraction
    let (block, ids) = worldobj::sky_table(&objs).expect("sky table");
    let sky_obj = objs
        .iter()
        .find_map(|o| match o {
            WorldObject::Brush { obj, brush } if brush.sky.is_some() => Some(*obj),
            _ => None,
        })
        .unwrap();
    assert_eq!(ids[0], sky_obj);
    assert!(ids[1..].iter().all(|&i| i == 0xFFFF));
    let f: Vec<f32> = block
        .chunks(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    assert_eq!(&f[..6], &[-1600.0, -200.0, -1600.0, 1600.0, 1200.0, 1600.0]);
    assert_eq!(&f[6..], &[-800.0, 325.0, -800.0, 800.0, 675.0, 800.0]);
    // brushes, volumes, the sprite
    let brushes = objs
        .iter()
        .filter(|o| matches!(o, WorldObject::Brush { .. }))
        .count();
    let volumes = objs
        .iter()
        .filter(|o| matches!(o, WorldObject::Volume { .. }))
        .count();
    let sprites = objs
        .iter()
        .filter(|o| matches!(o, WorldObject::Sprite { .. }))
        .count();
    assert_eq!((brushes, volumes, sprites), (3, 2, 1));
    let sounds = worldobj::plan_sounds(&w, WorldObjects::World, None, &mut id);
    assert_eq!(sounds.len(), 1);
    assert_eq!(
        sounds[0].files(),
        vec!["@mod:test/assets/wind.wav".to_string()]
    );
    // a TDM room: the flags are created but not shown
    let objs = worldobj::plan(&w, &tables, WorldObjects::Mode, ZoneClass::Room, 1, &mut id);
    assert!(objs
        .iter()
        .any(|o| matches!(o, WorldObject::Flag { shown: false, .. })));
}

/// The sim side of the same file: the ladder brush is a container volume; the collision set is the level model,
/// the world model and the breakable (not the kill volume, the ladder or the sky model); nothing is a mover.
#[test]
fn sim_reads_the_community_map() {
    let bytes = synthetic_dc_world_bytes();
    let w = dc_world::World::parse_with(&bytes, dc_world::ParseOptions { render: false }).unwrap();
    let ladders = dc_sim::from_world::ladder_volumes(&w);
    assert_eq!(ladders.volumes.len(), 1);
    let filter = dc_sim::from_world::WorldFilter::default();
    let picked = dc_sim::from_world::selected_models(&w, &filter);
    let names: Vec<&[u8]> = picked.iter().map(|&i| w.models[i].name).collect();
    assert_eq!(names, vec![&b""[..], b"crate_1", b"glass_1"]);
    assert!(w.objects.iter().all(|o| !filter.is_mover(o)));
}
