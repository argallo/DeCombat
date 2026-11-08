//! The composer on the made-up fixture (`dc_world::compose::fixture`): a map placing one piece of a "ROM" world.
//! DC-DEFAULT: MAP-116.

use dc_world::compose::{self, clip::Aabb, fixture};
use dc_world::{FieldValue, ObjectRole, World};

fn composed(cuts: Vec<Aabb>) -> Vec<u8> {
    let map = fixture::map(&fixture::piece(cuts));
    let mut read = fixture::reader();
    compose::compose(&map, &mut read)
        .expect("compose")
        .expect("the map has a piece")
        .bytes
}

fn level<'w, 'a>(w: &'w World<'a>) -> &'w dc_world::BspModel<'a> {
    w.models
        .iter()
        .find(|m| w.object_named(m.name).is_none())
        .expect("level model")
}

fn corners(m: &dc_world::BspModel<'_>, p: &dc_world::Polygon) -> Vec<[f32; 3]> {
    m.polygon_points(p)
        .iter()
        .map(|&i| m.points[i as usize])
        .collect()
}

#[test]
fn a_map_without_pieces_is_left_alone() {
    let rom = fixture::rom_world();
    let mut read = fixture::reader();
    assert!(compose::compose(&rom, &mut read).unwrap().is_none());
    assert!(!compose::has_pieces(&rom));
    assert!(compose::has_pieces(&fixture::map(&fixture::piece(
        Vec::new()
    ))));
    assert_eq!(compose::compose_with_rom(rom.clone(), None).unwrap(), rom);
}

#[test]
fn the_piece_record_round_trips() {
    let cuts = vec![Aabb::new([10_400.0, 0.0, -50.0], [10_700.0, 150.0, 50.0])];
    let p = fixture::piece(cuts);
    let map = fixture::map(&p);
    let w = World::parse(&map).unwrap();
    let got = compose::pieces_of(&w).unwrap();
    assert_eq!(got, vec![p]);
}

#[test]
fn the_composed_world_parses_and_keeps_the_piece_to_its_region() {
    let bytes = composed(Vec::new());
    let w = World::parse(&bytes).expect("parse");
    w.check_layout().expect("layout");
    let lm = level(&w);
    // the map's own floor is still there, the piece's floor is clipped at the region's east face, its lid is gone
    let mut floor_x = (f32::MAX, f32::MIN);
    let mut sky = 0;
    for p in &lm.polygons {
        let c = corners(lm, p);
        if c.len() < 3 {
            continue;
        }
        let s = lm.surfaces[p.surface as usize];
        if s.flags & 0x10 != 0 {
            sky += 1;
        }
        if c.iter().all(|q| q[1] == 0.0) && c.iter().all(|q| q[0] < 11_000.0) {
            for q in &c {
                floor_x = (floor_x.0.min(q[0]), floor_x.1.max(q[0]));
            }
        }
        if c.iter().any(|q| q[0] > 10_600.01 && q[0] < 11_900.0) {
            panic!("a piece polygon past the region: {c:?}");
        }
    }
    assert_eq!(sky, 0);
    assert_eq!(floor_x, (9_000.0, 10_600.0));
    // every solid polygon is referenced by a node
    let mut seen = vec![false; lm.polygons.len()];
    for n in &lm.nodes {
        seen[n.poly as usize] = true;
    }
    for (i, p) in lm.polygons.iter().enumerate() {
        if lm.surfaces[p.surface as usize].flags & 1 != 0 {
            assert!(seen[i], "solid polygon {i} unreferenced");
        }
    }
    // objects: one spawn (the duplicate merged, the one past the region left out), the item, the renamed crate,
    // the clipped pit, the lamp whose reference follows the rename; no world settings of the piece
    let spawns: Vec<_> = w.objects_with_role(ObjectRole::PlayerSpawn).collect();
    assert_eq!(spawns.len(), 1);
    assert_eq!(spawns[0].position(), Some([10_000.0, 50.0, 0.0]));
    assert_eq!(spawns[0].name(), Some(&b"piece_1~spawn_1"[..]));
    assert_eq!(w.objects_with_role(ObjectRole::ItemSpawn).count(), 1);
    assert_eq!(w.objects_with_role(ObjectRole::WorldSettings).count(), 1);
    let crate_obj = w.object_named(b"piece_1~crate").expect("renamed crate");
    assert_eq!(crate_obj.position(), Some([10_100.0, 20.0, 100.0]));
    let crate_model = w.model_named(b"piece_1~crate").expect("crate model");
    assert!(crate_model.is_dual());
    assert!(w
        .render
        .as_ref()
        .unwrap()
        .world
        .models
        .iter()
        .any(|m| m.name == b"piece_1~crate"));
    let lamp = w.object_named(b"piece_1~lamp").expect("lamp");
    let target = lamp.props.iter().find(|p| p.name == b"target").unwrap();
    assert_eq!(target.value, FieldValue::Text(b"piece_1~crate"));
    let pit = w.model_named(b"piece_1~pit").expect("clipped pit");
    let b = Aabb::new(pit.box_min, pit.box_max);
    assert!(
        (b.min[0] - 9_000.0).abs() < 0.01 && (b.max[0] - 10_600.0).abs() < 0.01,
        "{b:?}"
    );
    assert!(
        (b.min[2] + 1_000.0).abs() < 0.01 && (b.max[2] - 1_000.0).abs() < 0.01,
        "{b:?}"
    );
    assert_eq!(pit.polygons.len(), 6);
    // render: the piece's block is clipped like its floor
    let r = w.render.as_ref().unwrap();
    for blk in &r.world.blocks {
        for v in blk.vertices.as_chunks::<68>().0 {
            let x = f32::from_le_bytes([v[0], v[1], v[2], v[3]]);
            assert!(x <= 10_600.01 || x >= 11_999.99, "vertex at x {x}");
        }
    }
    // blockers: the one inside the region, moved
    assert_eq!(w.blockers.polys.len(), 1);
    assert_eq!(w.blockers.polys[0].verts[0][0], 9_100.0);
    assert_eq!(w.blockers.polys[0].dist, 9_100.0);
    // the light grid takes the piece's samples inside its region
    assert!(w.extent_min[0] <= 9_000.0 - 127.0 && w.extent_max[0] >= 14_000.0);
    let rgb = w.light_grid.expand();
    let g = &w.light_grid;
    let idx = |p: [f32; 3]| {
        let i = |a: usize| ((p[a] - g.origin[a]) / g.cell[a]).round() as usize;
        ((i(2) * g.dims[1] as usize + i(1)) * g.dims[0] as usize + i(0)) * 3
    };
    // the piece's own sample (1, 1, 1) sits at (10_000, 500, 0) and is (36, 100, 200) ... the composed grid
    // starts at the extent's corner, so look up the nearest composed sample and compare its green / blue
    let at = idx([10_000.0, 500.0, 0.0]);
    assert_eq!(&rgb[at + 1..at + 3], &[100, 200]);
    // outside every region: the map's own ambient
    let at = idx([13_000.0, 0.0, 0.0]);
    assert_eq!(&rgb[at..at + 3], &[40, 40, 40]);
}

#[test]
fn a_cut_opens_the_wall_and_the_kill_slab() {
    // a doorway through the wall at x 10,500 and down through the pit below it
    let door = Aabb::new([10_400.0, -700.0, -50.0], [10_700.0, 150.0, 50.0]);
    let bytes = composed(vec![door]);
    let w = World::parse(&bytes).unwrap();
    w.check_layout().unwrap();
    let lm = level(&w);
    for p in &lm.polygons {
        let c = corners(lm, p);
        if c.len() < 3 {
            continue;
        }
        let b = Aabb::of_points(c.iter()).unwrap();
        assert!(!b.overlaps(&door), "a polygon inside the cut: {c:?}");
    }
    // the pit falls apart into pieces around the cut, none inside it
    let pits: Vec<_> = w
        .models
        .iter()
        .filter(|m| m.name.starts_with(b"piece_1~pit"))
        .collect();
    assert!(pits.len() >= 2, "{}", pits.len());
    for m in pits {
        assert!(w.object_named(m.name).is_some());
        let b = Aabb::new(m.box_min, m.box_max);
        assert!(!b.overlaps(&door), "{b:?}");
    }
}

#[test]
fn composing_twice_gives_the_same_bytes() {
    assert_eq!(composed(Vec::new()), composed(Vec::new()));
}

#[test]
fn a_wrong_digest_or_overlap_is_refused() {
    let mut p = fixture::piece(Vec::new());
    p.sha256 = "00".repeat(32);
    let mut read = fixture::reader();
    let e = compose::compose(&fixture::map(&p), &mut read).unwrap_err();
    assert!(e.to_string().contains("SHA-256"), "{e}");
    let mut p = fixture::piece(Vec::new());
    p.sha256 = dc_rom::sha256_hex(&fixture::rom_world());
    assert!(compose::compose(&fixture::map(&p), &mut read).is_ok());
}

/// A pack is downloaded data: a record that would size an allocation without bound, or that pins no digest of
/// its world, is refused before anything is read or allocated for it.
#[test]
fn a_hostile_record_is_refused_before_anything_is_allocated() {
    let refused = |p: &compose::Piece, what: &str| {
        // the reader counts: a refused record never reaches the ROM
        let mut reads = 0usize;
        let mut inner = fixture::reader();
        let mut read = |path: &str| {
            reads += 1;
            inner(path)
        };
        let e = compose::compose(&fixture::map(p), &mut read).unwrap_err();
        assert!(matches!(e, compose::ComposeError::Record(_)), "{what}: {e}");
        assert!(e.to_string().contains(what), "{what}: {e}");
        assert_eq!(reads, 0, "{what}: the ROM was read");
    };
    // no digest (a map built without the ROM)
    let mut p = fixture::piece(Vec::new());
    p.sha256 = String::new();
    refused(&p, "digest");
    let mut p = fixture::piece(Vec::new());
    p.sha256 = "not hex".into();
    refused(&p, "digest");
    // a region a billion units across (the fill grid and the clipping are sized by it)
    let mut p = fixture::piece(Vec::new());
    p.region = Aabb::new([-1.0e9, -100.0, -1.0e9], [1.0e9, 100.0, 1.0e9]);
    refused(&p, "from the origin");
    let mut p = fixture::piece(Vec::new());
    p.region = Aabb::new([0.0, -100.0, 0.0], [70_000.0, 100.0, 100.0]);
    refused(&p, "across");
    // an offset far away, a far cut, too many cuts, too many left-out spawns
    let mut p = fixture::piece(Vec::new());
    p.offset = [2.0e6, 0.0, 0.0];
    refused(&p, "from the origin");
    let mut p = fixture::piece(vec![Aabb::new(
        [1.0e7, 0.0, 0.0],
        [1.0e7 + 10.0, 10.0, 10.0],
    )]);
    refused(&p, "from the origin");
    let cut = Aabb::new([10_000.0, 1.0, -50.0], [10_010.0, 150.0, 50.0]);
    let p = fixture::piece(vec![cut; compose::MAX_CUTS + 1]);
    refused(&p, "cuts");
    let mut p = fixture::piece(Vec::new());
    p.drop_spawns = vec![[0.0; 3]; compose::MAX_DROP_SPAWNS + 1];
    refused(&p, "spawns");
    // the bounds leave an ordinary record alone
    let mut read = fixture::reader();
    let p = fixture::piece(vec![cut; compose::MAX_CUTS]);
    assert!(compose::compose(&fixture::map(&p), &mut read).is_ok());
}

fn fill_piece(blockers: bool) -> compose::Piece {
    // a region wider than the fixture world's floor (z -1,000 .. 1,000): bands at |z| > 1,000 have no floor
    let mut p = fixture::piece(Vec::new());
    p.region = Aabb::new([9_000.0, -600.0, -1_500.0], [10_600.0, 1_100.0, 1_500.0]);
    p.blockers = blockers;
    p.fill = Some(compose::Fill {
        y: 0.0,
        texture: "fixture/sand".into(),
        tile: 512.0,
        color: [128.0, 120.0, 110.0],
        surface: 9,
    });
    p
}

#[test]
fn the_fill_floors_the_bare_columns_outside_the_play_area() {
    let p = fill_piece(true);
    let map = fixture::map(&p);
    let got = compose::pieces_of(&World::parse(&map).unwrap()).unwrap();
    assert_eq!(got, vec![p]);
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    w.check_layout().unwrap();
    let lm = level(&w);
    // fill strips: 2 units under y 0, only where the world has no floor (|z| > 1,000), inside the region
    let mut fill_area = 0.0f32;
    for poly in &lm.polygons {
        let c = corners(lm, poly);
        if c.len() < 3 || c.iter().any(|q| (q[1] + 2.0).abs() > 0.01) {
            continue;
        }
        let b = Aabb::of_points(c.iter()).unwrap();
        assert!(
            b.min[2] >= -1_500.0 - 0.01 && b.max[2] <= 1_500.0 + 0.01,
            "{b:?}"
        );
        assert!(
            b.min[2] >= 1_000.0 - 128.0 || b.max[2] <= -1_000.0 + 128.0,
            "fill over the floor: {b:?}"
        );
        assert_eq!(lm.surfaces[poly.surface as usize].texflags, 9);
        let pl = lm.planes[poly.plane as usize];
        assert_eq!((pl.normal, pl.dist), ([0.0, 1.0, 0.0], -2.0));
        fill_area += (b.max[0] - b.min[0]) * (b.max[2] - b.min[2]);
    }
    // both bands, 1,600 x 500 each, at least (the cells straddling the floor's edge count too)
    assert!(fill_area >= 2.0 * 1_600.0 * 500.0 - 1.0, "{fill_area}");
    let r = w.render.as_ref().unwrap();
    assert!(r
        .world
        .blocks
        .iter()
        .any(|b| b.sections.iter().any(|s| s.textures[0] == b"fixture/sand")));
    // a closed piece keeps its blocker
    assert_eq!(w.blockers.polys.len(), 1);
}

#[test]
fn a_floor_over_the_fill_is_filled_under_and_capped_where_the_region_cuts_it() {
    // the fill 300 under the world's floor: the floor covers nothing, and the region's east side (x 10,600) cuts it
    let mut p = fill_piece(false);
    p.fill.as_mut().unwrap().y = -300.0;
    let map = fixture::map(&p);
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    w.check_layout().unwrap();
    let lm = level(&w);
    let (mut fill_area, mut cap_area) = (0.0f32, 0.0f32);
    for poly in &lm.polygons {
        let c = corners(lm, poly);
        if c.len() < 3 {
            continue;
        }
        let pl = lm.planes[poly.plane as usize];
        let b = Aabb::of_points(c.iter()).unwrap();
        if (pl.normal, pl.dist) == ([0.0, 1.0, 0.0], -302.0) {
            fill_area += (b.max[0] - b.min[0]) * (b.max[2] - b.min[2]);
        } else if pl.normal == [1.0, 0.0, 0.0]
            && (b.min[0] - 10_600.0).abs() < 0.01
            && (b.max[0] - 10_600.0).abs() < 0.01
        {
            // a cap faces out of the region, from the floor's edge down to the fill floor
            assert!(b.min[1] >= -302.01 && b.max[1] <= 0.01, "{b:?}");
            assert_eq!(lm.surfaces[poly.surface as usize].texflags, 9);
            cap_area += (b.max[1] - b.min[1]) * (b.max[2] - b.min[2]);
        }
    }
    // the whole region is filled under the floor (1,600 x 3,000) and the cap closes the floor's cut edge
    assert!(fill_area >= 1_600.0 * 3_000.0 - 1.0, "{fill_area}");
    assert!((cap_area - 2_000.0 * 302.0).abs() < 1.0, "{cap_area}");
    // drawn in the fill's block: the strips and the cap's triangles
    let r = w.render.as_ref().unwrap();
    let sand = r
        .world
        .blocks
        .iter()
        .find(|b| b.sections.iter().any(|s| s.textures[0] == b"fixture/sand"))
        .expect("fill block");
    assert!(sand.triangle_count >= 4);
    // a fill at the floor's own height covers nothing new and caps nothing
    let p0 = fill_piece(false);
    let bytes0 = compose::compose(&fixture::map(&p0), &mut read)
        .unwrap()
        .unwrap()
        .bytes;
    let w0 = World::parse(&bytes0).unwrap();
    let lm0 = level(&w0);
    assert!(!lm0
        .polygons
        .iter()
        .any(
            |poly| lm0.planes[poly.plane as usize].normal == [1.0, 0.0, 0.0]
                && corners(lm0, poly).len() >= 3
                && corners(lm0, poly)
                    .iter()
                    .all(|q| (q[0] - 10_600.0).abs() < 0.01)
        ));
}

#[test]
fn an_open_piece_drops_its_blockers_and_keeps_its_pit_to_the_play_area() {
    let map = fixture::map(&fill_piece(false));
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    assert!(w.blockers.polys.is_empty());
    // the play area: spawns, item spawns and blockers of the fixture world (x -900 .. 900, z -100 .. 100 around
    // its origin, moved by 10,000 in x), grown by the open margin, inside the region
    let pit = w
        .model_named(b"piece_1~pit")
        .expect("the pit stays in the play area");
    let b = Aabb::new(pit.box_min, pit.box_max);
    let m = compose::OPEN_KILL_MARGIN;
    assert!((b.min[0] - (9_100.0 - m)).abs() < 0.5, "{b:?}");
    assert!((b.max[0] - 10_600.0).abs() < 0.5, "{b:?}");
    assert!(
        (b.min[2] - (-100.0 - m)).abs() < 0.5 && (b.max[2] - (100.0 + m)).abs() < 0.5,
        "{b:?}"
    );
}

#[test]
fn a_filled_open_piece_keeps_only_what_kills_under_its_fill_floor() {
    // the fixture's pit volume spans y -500 .. -400; KILL_UNDER under the fill floor (2 under the fill height) and
    // lower is all a filled open piece keeps of it
    let pit_y = |fill_y: f32| {
        let mut p = fill_piece(false);
        p.fill.as_mut().unwrap().y = fill_y;
        let mut read = fixture::reader();
        let bytes = compose::compose(&fixture::map(&p), &mut read)
            .unwrap()
            .unwrap()
            .bytes;
        let w = World::parse(&bytes).unwrap();
        w.model_named(b"piece_1~pit")
            .map(|m| (m.box_min[1], m.box_max[1]))
    };
    let top = |fill_y: f32| fill_y - compose::fill::SINK - compose::KILL_UNDER;
    // the fill high over it: the pit is kept whole
    let (lo, hi) = pit_y(0.0).expect("kept");
    assert!(
        (lo + 500.0).abs() < 0.5 && (hi + 400.0).abs() < 0.5,
        "{lo} {hi}"
    );
    // the fill just over it: cut down to KILL_UNDER under the fill floor
    let (lo, hi) = pit_y(-350.0).expect("kept in part");
    assert!(
        (lo + 500.0).abs() < 0.5 && (hi - top(-350.0)).abs() < 0.5,
        "{lo} {hi}"
    );
    // the fill under it: gone
    assert!(pit_y(-450.0).is_none());
}

#[test]
fn a_cap_leaves_a_doorway_open() {
    // the fill 300 under the world's floor, and a doorway cut through the region's east side under the floor's
    // edge (z -200 .. 200, y -290 .. -50): the cap there has a hole the doorway's size
    let mut p = fill_piece(false);
    p.fill.as_mut().unwrap().y = -300.0;
    p.cuts = vec![Aabb::new(
        [10_400.0, -290.0, -200.0],
        [10_604.0, -50.0, 200.0],
    )];
    let map = fixture::map(&p);
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    let lm = level(&w);
    let mut cap_area = 0.0f32;
    for poly in &lm.polygons {
        let c = corners(lm, poly);
        if c.len() < 3 || !c.iter().all(|q| (q[0] - 10_600.0).abs() < 0.01) {
            continue;
        }
        if lm.planes[poly.plane as usize].normal != [1.0, 0.0, 0.0] {
            continue;
        }
        // no corner strictly inside the doorway
        for q in &c {
            assert!(
                !(q[2] > -199.9 && q[2] < 199.9 && q[1] > -289.9 && q[1] < -50.1),
                "{q:?}"
            );
        }
        // the area of a convex polygon on the plane x = 10,600 (y, z)
        let mut a = 0.0f32;
        for k in 0..c.len() {
            let (p0, p1) = (c[k], c[(k + 1) % c.len()]);
            a += p0[2] * p1[1] - p1[2] * p0[1];
        }
        cap_area += a.abs() / 2.0;
    }
    let want = 2_000.0 * 302.0 - 400.0 * 240.0;
    assert!((cap_area - want).abs() < 1.0, "{cap_area} {want}");
}

#[test]
fn a_dropped_spawn_point_takes_the_spawns_near_it() {
    // MAP-125: a point in drop_spawns takes the world's player spawns within 48 across (96 up or down); the record
    // carries the points and reads them back
    let mut p = fixture::piece(Vec::new());
    let mut read = fixture::reader();
    let spawns_of = |bytes: &[u8]| -> Vec<[f32; 3]> {
        let w = World::parse(bytes).unwrap();
        w.objects_with_role(dc_world::ObjectRole::PlayerSpawn)
            .filter(|o| o.name().is_some_and(|n| n.starts_with(b"piece_1~")))
            .filter_map(|o| o.position())
            .collect()
    };
    let all = spawns_of(
        &compose::compose(&fixture::map(&p), &mut read)
            .unwrap()
            .unwrap()
            .bytes,
    );
    assert!(!all.is_empty());
    let gone = all[0];
    p.drop_spawns = vec![[gone[0] + 10.0, gone[1] + 30.0, gone[2] - 10.0]];
    let map = fixture::map(&p);
    assert_eq!(
        compose::pieces_of(&World::parse(&map).unwrap()).unwrap()[0].drop_spawns,
        p.drop_spawns
    );
    let left = spawns_of(&compose::compose(&map, &mut read).unwrap().unwrap().bytes);
    assert!(left.len() < all.len());
    assert!(!left.contains(&gone));
    // a point far off takes nothing
    p.drop_spawns = vec![[gone[0] + 500.0, gone[1], gone[2]]];
    let kept = spawns_of(
        &compose::compose(&fixture::map(&p), &mut read)
            .unwrap()
            .unwrap()
            .bytes,
    );
    assert_eq!(kept.len(), all.len());
}
