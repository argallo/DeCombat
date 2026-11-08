//! The composer on the user's own ROM worlds (DC-DEFAULT: MAP-116). Skipped unless CA_ROM names a client folder.
//! The two smallest worlds go side by side as pieces of a made-up map: the composed world parses and tiles, has one
//! level model, keeps every moved blocker on its plane, and comes out byte for byte the same twice. Every ROM world
//! on its own passes the loaders' hook unchanged. Counts only; nothing of the ROM is printed or kept.

use dc_world::compose::{self, clip::Aabb, fixture, Piece, Spawns};
use dc_world::synth::{SynthObject, SynthRenderWorld, SynthValue, SynthWorld};
use dc_world::World;

#[test]
fn two_rom_worlds_compose_side_by_side() {
    let Ok(rom) = dc_rez::dc_rom::resolve_rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let mut worlds: Vec<(String, u64)> = vfs
        .list_entries("WORLDS.rez")
        .expect("world archive")
        .into_iter()
        .filter(|e| e.0.to_ascii_uppercase().ends_with(".DAT"))
        .map(|e| (e.0, e.2 as u64))
        .collect();
    worlds.sort_by_key(|w| w.1);
    let mut read = compose::rom_reader(&rom);
    let mut pieces = Vec::new();
    let mut x = 0.0f32;
    for (k, (path, _)) in worlds.iter().take(2).enumerate() {
        let bytes = read(path).expect("read world");
        let w = World::parse(&bytes).expect("parse world");
        let info = compose::piece_info(&w);
        let play = info.play.expect("a play box");
        let region = Aabb::new(
            [
                play.min[0] - 500.0,
                play.min[1] - 800.0,
                play.min[2] - 500.0,
            ],
            [
                play.max[0] + 500.0,
                play.max[1] + 800.0,
                play.max[2] + 500.0,
            ],
        );
        let offset = [x - region.min[0], 0.0, -region.min[2]];
        x += region.max[0] - region.min[0] + 1000.0;
        pieces.push(Piece {
            name: format!("piece_{}", k + 1),
            world: path.clone(),
            sha256: dc_rez::dc_rom::sha256_hex(&bytes),
            offset,
            region: region.translated(offset),
            cuts: Vec::new(),
            spawns: Spawns::Ffa,
            items: true,
            sky: k == 0,
            blockers: true,
            fill: None,
            min_light: 0,
            drop_spawns: Vec::new(),
        });
    }
    let floor = fixture::Face {
        corners: vec![
            [-100.0, -3000.0, -100.0],
            [-100.0, -3000.0, 100.0],
            [100.0, -3000.0, 100.0],
            [100.0, -3000.0, -100.0],
        ],
        flags: 1,
        texflags: 7,
    };
    let mut objects = vec![SynthObject::dc(
        "dc.world_settings",
        "world_settings",
        [0.0; 3],
        [0.0; 3],
        &[("far_z", SynthValue::Real(20_000.0))],
    )];
    objects.extend(pieces.iter().map(Piece::to_object));
    let map = SynthWorld {
        info: b"rom compose test".to_vec(),
        extent_min: [-200.0, -3100.0, -200.0],
        extent_max: [200.0, -2900.0, 200.0],
        tree_shape: vec![false],
        models: vec![fixture::model("level", 0, [0.0; 3], &[floor.clone()])],
        objects,
        grid_origin: [-200.0, -3100.0, -200.0],
        grid_cell: [400.0, 200.0, 400.0],
        grid_dims: [1, 1, 1],
        grid_rgb: vec![60, 60, 60],
        render: SynthRenderWorld {
            blocks: vec![fixture::block(&[floor], b"fixture/grass")],
            models: Vec::new(),
        },
        ..SynthWorld::default()
    }
    .build();
    let a = compose::compose(&map, &mut read)
        .expect("compose")
        .expect("pieces");
    let b = compose::compose(&map, &mut read)
        .expect("compose")
        .expect("pieces");
    assert_eq!(a.bytes, b.bytes, "composing twice gives the same bytes");
    let w = World::parse(&a.bytes).expect("parse composed");
    w.check_layout().expect("layout");
    let named: std::collections::BTreeSet<&[u8]> =
        w.objects.iter().filter_map(|o| o.name()).collect();
    assert_eq!(
        w.models.iter().filter(|m| !named.contains(m.name)).count(),
        1,
        "one level model"
    );
    assert_eq!(a.stats.pieces, 2);
    assert!(a.stats.player_spawns > 0 && a.stats.triangles > 0);
    // every blocker corner lies on its plane, as in the ROM (the offset moved the plane with it)
    for p in &w.blockers.polys {
        for v in &p.verts {
            let d = p.normal[0] * v[0] + p.normal[1] * v[1] + p.normal[2] * v[2] - p.dist;
            assert!(d.abs() < 1.0, "a blocker corner {d} off its plane");
        }
    }
    // every piece's geometry stays in its region (the level's corners, the render vertices)
    let regions: Vec<Aabb> = pieces.iter().map(|p| p.region).collect();
    let inside = |q: [f32; 3]| q[1] <= -2999.0 || regions.iter().any(|r| r.contains(q));
    let level = w.models.iter().find(|m| !named.contains(m.name)).unwrap();
    assert!(
        level.points.iter().all(|&q| inside(q)),
        "level corner outside every region"
    );
    for blk in &w.render.as_ref().unwrap().world.blocks {
        for v in blk.vertices.as_chunks::<68>().0 {
            let f = |o: usize| f32::from_le_bytes([v[o], v[o + 1], v[o + 2], v[o + 3]]);
            let q = [f(0), f(4), f(8)];
            assert!(inside(q), "render vertex outside every region");
        }
    }
}

/// Inertness (the hand-back's proof): every world of the user's ROM is a map without pieces, so the step every
/// loader runs on the bytes it read (`compose_with_rom`, with the ROM at hand) gives the bytes back unchanged, and
/// `compose` finds nothing to do. Skipped unless CA_ROM names a client folder. Counts only; nothing of the ROM is
/// printed or kept.
#[test]
fn every_rom_world_is_left_alone_by_the_loaders_hook() {
    let Ok(rom) = dc_rez::dc_rom::resolve_rom() else {
        eprintln!("CA_ROM not set: skipped");
        return;
    };
    let vfs = dc_rez::RezVfs::new(&rom);
    let worlds: Vec<String> = vfs
        .list_entries("WORLDS.rez")
        .expect("world archive")
        .into_iter()
        .map(|e| e.0)
        .filter(|p| p.to_ascii_uppercase().ends_with(".DAT"))
        .collect();
    assert!(!worlds.is_empty(), "no worlds in the archive");
    let mut read = compose::rom_reader(&rom);
    let mut checked = 0;
    for path in &worlds {
        let bytes = read(path).expect("read world");
        if World::parse_with(&bytes, dc_world::ParseOptions { render: false }).is_err() {
            continue; // not a version-85 world (the archive holds other files too)
        }
        assert!(
            !compose::has_pieces(&bytes),
            "a ROM world holds no dc.rom_piece object"
        );
        assert!(
            compose::compose(&bytes, &mut read)
                .expect("compose")
                .is_none(),
            "compose has nothing to do on a ROM world"
        );
        let out = compose::compose_with_rom(bytes.clone(), Some(&rom)).expect("compose_with_rom");
        assert_eq!(out.len(), bytes.len(), "world {checked}: size changed");
        assert!(out == bytes, "world {checked}: bytes changed");
        checked += 1;
    }
    assert!(checked > 0, "no version-85 world checked");
    eprintln!("{checked} ROM worlds pass through compose_with_rom unchanged");
}
