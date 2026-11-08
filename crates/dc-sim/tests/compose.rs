//! Collision on a composed world (DC-DEFAULT: MAP-116): the fixture map with one piece of a made-up "ROM" world
//! (`dc_world::compose::fixture`), swept by the game's own walk. Inside the piece's region its geometry stops
//! rays, past the region it is gone, the map's own solids inside the region still stop rays, a sweep across a
//! region face finds what lies on either side, and a cut opens a doorway.

use dc_sim::from_world::{collision_world, WorldFilter};
use dc_sim::vec3::V3;
use dc_sim::CollisionWorld;
use dc_world::compose::{self, clip::Aabb, fixture};
use dc_world::World;

fn world(cuts: Vec<Aabb>) -> CollisionWorld {
    let map = fixture::map(&fixture::piece(cuts));
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    collision_world(&w, &WorldFilter::default())
}

fn v(x: f32, y: f32, z: f32) -> V3 {
    V3::new(x, y, z)
}

#[test]
fn the_piece_floor_stops_a_ray_inside_its_region_only() {
    let c = world(Vec::new());
    let hit = c
        .ray(v(10_000.0, 300.0, 0.0), v(10_000.0, -300.0, 0.0))
        .expect("piece floor");
    assert!((hit.t - 0.5).abs() < 0.01, "{}", hit.t);
    // past the region's east face (x 10,600) the piece's floor is cut away; the map's own floor starts at 12,000
    assert!(c
        .ray(v(10_800.0, 300.0, 0.0), v(10_800.0, -300.0, 0.0))
        .is_none());
    let hit = c
        .ray(v(13_000.0, 300.0, 0.0), v(13_000.0, -300.0, 0.0))
        .expect("map floor");
    assert!((hit.t - 0.5).abs() < 0.01);
}

#[test]
fn the_maps_own_cover_inside_a_region_still_stops_rays() {
    let c = world(Vec::new());
    let hit = c
        .ray(v(10_100.0, 50.0, 320.0), v(10_300.0, 50.0, 320.0))
        .expect("own cover");
    assert!((hit.t - 0.4).abs() < 0.01, "{}", hit.t);
}

#[test]
fn a_ray_across_a_region_face_finds_the_wall_inside() {
    let c = world(Vec::new());
    // from inside the region out past its face: the wall at x 10,500 (facing -x)
    let hit = c
        .ray(v(10_000.0, 100.0, 0.0), v(13_000.0, 100.0, 0.0))
        .expect("wall");
    assert!((hit.t - 500.0 / 3_000.0).abs() < 0.01, "{}", hit.t);
}

#[test]
fn a_cut_opens_a_doorway_through_the_wall() {
    let door = Aabb::new([10_400.0, 1.0, -50.0], [10_700.0, 150.0, 50.0]);
    let c = world(vec![door]);
    assert!(c
        .ray(v(10_000.0, 100.0, 0.0), v(10_590.0, 100.0, 0.0))
        .is_none());
    // beside the doorway and above it the wall stands
    assert!(c
        .ray(v(10_000.0, 100.0, 300.0), v(10_590.0, 100.0, 300.0))
        .is_some());
    assert!(c
        .ray(v(10_000.0, 180.0, 0.0), v(10_590.0, 180.0, 0.0))
        .is_some());
    // the floor is still there in the doorway (the cut starts above it)
    assert!(c
        .ray(v(10_450.0, 100.0, 0.0), v(10_450.0, -100.0, 0.0))
        .is_some());
}

#[test]
fn the_fill_floor_stops_a_ray_where_the_world_has_no_floor() {
    let mut p = fixture::piece(Vec::new());
    p.region = Aabb::new([9_000.0, -600.0, -1_500.0], [10_600.0, 1_100.0, 1_500.0]);
    p.fill = Some(compose::Fill {
        y: 0.0,
        texture: "fixture/sand".into(),
        tile: 512.0,
        color: [128.0; 3],
        surface: 9,
    });
    let map = fixture::map(&p);
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    let c = collision_world(&w, &WorldFilter::default());
    let hit = c
        .ray(v(10_000.0, 300.0, 1_300.0), v(10_000.0, -300.0, 1_300.0))
        .expect("fill");
    assert!((300.0 - 600.0 * hit.t - (-2.0)).abs() < 0.5, "{}", hit.t);
    // the world's own floor is still what stops it over the floor
    let hit = c
        .ray(v(10_000.0, 300.0, 500.0), v(10_000.0, -300.0, 500.0))
        .expect("floor");
    assert!((hit.t - 0.5).abs() < 0.01);
}

#[test]
fn a_cap_stops_a_player_walking_under_a_cut_off_floor() {
    // the fill 300 under the world's floor: walking in at the fill's height from outside the region's east side,
    // a player meets the cap under the floor's cut edge, and stands on the fill under the floor
    let mut p = fixture::piece(Vec::new());
    p.region = Aabb::new([9_000.0, -600.0, -1_500.0], [10_600.0, 1_100.0, 1_500.0]);
    p.blockers = false;
    p.fill = Some(compose::Fill {
        y: -300.0,
        texture: "fixture/sand".into(),
        tile: 512.0,
        color: [128.0; 3],
        surface: 9,
    });
    let map = fixture::map(&p);
    let mut read = fixture::reader();
    let bytes = compose::compose(&map, &mut read).unwrap().unwrap().bytes;
    let w = World::parse(&bytes).unwrap();
    let c = collision_world(&w, &WorldFilter::default());
    let hit = c
        .ray(v(10_900.0, -150.0, 0.0), v(10_300.0, -150.0, 0.0))
        .expect("cap");
    assert!(
        (10_900.0 - 600.0 * hit.t - 10_600.0).abs() < 0.5,
        "{}",
        hit.t
    );
    let hit = c
        .ray(v(10_000.0, -100.0, 0.0), v(10_000.0, -500.0, 0.0))
        .expect("fill under the floor");
    assert!((-100.0 - 400.0 * hit.t - (-302.0)).abs() < 0.5, "{}", hit.t);
}
