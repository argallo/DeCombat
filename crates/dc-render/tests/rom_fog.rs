//! ROM-gated (round 10, VIS-20): the fog each world gets from its world-settings object. Snow Valley's sky is fogged
//! with the world's colour over the sky range (the engine's sky pass keeps the world's fog enable), a world whose
//! first fog-carrying object is not its world-settings object still gets its fog, and Junk Flea's world fog is the
//! one it had. Skipped when CA_ROM is unset.

use dc_render::{world_fog, Fog, WorldSource};

fn fog_of(src: &WorldSource, map: &str) -> (Option<Fog>, Option<Fog>) {
    let bytes = src.read(map).expect("read").expect("world present");
    let w = dc_world::World::parse(&bytes).expect("parse");
    world_fog(&w)
}

#[test]
fn rom_worlds_take_the_fog_of_their_world_settings_object() {
    let rom = dc_rom::rom_or_skip!();
    let src = WorldSource::open(&rom).expect("worlds");
    // Snow Valley: the sky pass is fogged (its own enable is off in the file) with the world's colour
    let (world, sky) = fog_of(&src, "SNOWVALLEY");
    let world = world.expect("Snow Valley world fog");
    let sky = sky.expect("Snow Valley sky fog follows the world's enable");
    assert_eq!(sky.colour, world.colour);
    assert!(sky.far > sky.near && sky.far < world.far, "{sky:?}");
    // a world whose earlier objects carry a switched-off fog of their own: the world-settings object wins
    let (world, _) = fog_of(&src, "RAPIDSTREAM");
    assert!(world.is_some(), "the world-settings object's fog");
    // Junk Flea's world fog stays what it was
    let (world, sky) = fog_of(&src, "JUNK_FLEA");
    let world = world.expect("Junk Flea world fog");
    assert_eq!((world.near, world.far), (600.0, 5000.0));
    assert_eq!(world.colour, [103.0, 101.0, 78.0]);
    assert_eq!(sky.map(|s| s.colour), Some(world.colour));
    // every world of the ROM has a world fog (each world-settings object switches it on)
    for m in src.map_names() {
        assert!(fog_of(&src, &m).0.is_some(), "{m}: no world fog");
    }
}
